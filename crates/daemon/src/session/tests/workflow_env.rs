// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Claude agents kept from their project's workflow variables until you
//! allow them.

use maxhq_protocol::model::AllowedWorkflowEnv;
use maxhq_protocol::timeline::{Question, QuestionAnswer};

use super::*;
use crate::session::workflow_env::{ALLOW_LABEL, NOT_NOW_LABEL, QUESTION, REMEMBER};
use crate::workflow::env::Var;

const TITLE: &str = "Give this agent the workflow's variables?";

/// Writes a descriptor with id `id` that names `env` for the agents in
/// `folder`.
fn describe(folder: &Path, id: &str, env: &str) {
    let workflows = folder.join(maxhq_workflow::WORKFLOWS_DIR);
    fs::create_dir_all(&workflows).expect("the folder is made");
    fs::write(
        workflows.join(format!("{id}.yaml")),
        format!(
            "format: maxhq.workflow/1.0\nid: {id}\nruns:\n  glob: .runs/*/run.yaml\n  env: {{ {env} }}\n"
        ),
    )
    .expect("the descriptor is written");
}

/// The value a launch sets `name` to, if it sets it.
fn launch_var(launch: &maxhq_hold::Launch, name: &str) -> Option<String> {
    launch
        .env
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.to_string_lossy().into_owned())
}

fn pick(key: IdempotencyKey, event: AttentionId, label: &str, remember: bool) -> ClientMessage {
    let mut answers = vec![QuestionAnswer {
        id: QUESTION.into(),
        values: vec![label.into()],
    }];
    if remember {
        answers.push(QuestionAnswer {
            id: REMEMBER.into(),
            values: vec!["Remember for this project".into()],
        });
    }
    ClientMessage::Answer {
        idempotency_key: key,
        event,
        answer: AttentionAnswer::Questions { answers },
        refs: Vec::new(),
    }
}

/// The workflow variable cards open now.
fn env_cards(h: &Harness) -> Vec<AttentionEvent> {
    h.hub
        .snapshot()
        .entities
        .into_iter()
        .filter_map(|entity| match entity {
            Entity::Attention(event) if event.summary.starts_with(TITLE) => Some(event),
            _ => None,
        })
        .collect()
}

/// Waits for the one card there should be.
async fn env_card(h: &Harness) -> AttentionEvent {
    eventually("a card", || !env_cards(h).is_empty()).await;
    let cards = env_cards(h);
    assert_eq!(cards.len(), 1, "{cards:#?}");
    cards.into_iter().next().expect("there's one")
}

async fn no_env_card(h: &Harness) {
    tokio::time::sleep(QUIET).await;
    assert_eq!(env_cards(h), Vec::new());
}

async fn answer(h: &mut Harness, event: AttentionId, label: &str, remember: bool) -> AnswerOutcome {
    let key = IdempotencyKey::generate();
    h.phone.send(pick(key, event, label, remember));
    h.phone.answered(key).await
}

/// The question the agent's timeline shows for its card last, with what
/// it was answered.
async fn env_question(h: &mut Harness, agent: AgentId) -> (Question, Option<Vec<QuestionAnswer>>) {
    let items = match h.open(agent, None, 200).await {
        TimelineUpdate::Reset { items, .. } => items,
        other => panic!("expected a reset, got {other:?}"),
    };
    items
        .iter()
        .rev()
        .find_map(|item| match &item.body {
            ItemBody::Question {
                questions, answers, ..
            } if questions.first().is_some_and(|q| q.id == QUESTION) => {
                Some((questions[0].clone(), answers.clone()))
            }
            _ => None,
        })
        .expect("the timeline has the card's question")
}

fn project(h: &Harness, agent: AgentId) -> ProjectId {
    match h.hub.get(&EntityKey::Workspace(h.agent(agent).workspace)) {
        Some(Entity::Workspace(workspace)) => workspace.project.expect("a folder has a project"),
        other => panic!("no workspace: {other:?}"),
    }
}

fn settings(project: ProjectId, on: bool, allowed: Option<AllowedWorkflowEnv>) -> Entity {
    Entity::ProjectSettings(ProjectSettings {
        project,
        device: device().id,
        resume_on_restart: true,
        ignored_ports: Vec::new(),
        git_author: None,
        auto_settle_days: None,
        settle_on_merge: true,
        workflow_env: on,
        memory_soft: None,
        workflow_env_allowed: allowed,
    })
}

fn busy(h: &Harness, agent: AgentId, activity: Activity) -> Say {
    Say {
        agents: vec![h.reported(agent, activity)],
        ..Say::default()
    }
}

#[tokio::test]
async fn a_folder_without_workflow_variables_asks_nothing() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let launch = h.holders.launch(session);
    assert_eq!(launch_var(&launch, "MAXHQ_AGENT"), Some(agent.to_string()));
    no_env_card(&h).await;
}

#[tokio::test]
async fn a_first_start_goes_without_the_variables_and_asks_for_them() {
    let mut h = Harness::new();
    describe(&h.folder, "example", "EXAMPLE_MODE: on");
    let (agent, session) = h.start_claude(None).await;
    let launch = h.holders.launch(session);
    assert_eq!(launch_var(&launch, "MAXHQ_AGENT"), Some(agent.to_string()));
    assert_eq!(launch_var(&launch, "EXAMPLE_MODE"), None);

    let card = env_card(&h).await;
    assert_eq!(card.agent, agent);
    assert_eq!(card.kind, AttentionKind::Question);
    // No buttons, so a notification can't answer it.
    assert_eq!(card.options, Vec::new());
    assert!(card.summary.contains("example"), "{}", card.summary);
}

#[tokio::test]
async fn a_descriptor_built_to_stall_the_scanner_costs_an_agents_start_nothing() {
    let mut h = Harness::new();
    let workflows = h.folder.join(maxhq_workflow::WORKFLOWS_DIR);
    fs::create_dir_all(&workflows).unwrap();
    let deep = 30_000;
    fs::write(
        workflows.join("bad.yaml"),
        format!("x: {}{}\n", "[".repeat(deep), "]".repeat(deep)),
    )
    .unwrap();
    describe(&h.folder, "example", "EXAMPLE_MODE: on");
    let started = std::time::Instant::now();
    let (agent, _) = h.start_claude(None).await;
    // Unchecked, the scanner spends many seconds here.
    assert!(started.elapsed() < Duration::from_secs(5));
    env_card(&h).await;
    let (question, _) = env_question(&mut h, agent).await;
    assert!(
        question.text.contains("EXAMPLE_MODE=on"),
        "{}",
        question.text
    );
}

#[tokio::test]
async fn the_card_lists_each_variable_and_marks_secrets_and_risky_kinds() {
    let mut h = Harness::new();
    describe(
        &h.folder,
        "example",
        "EXAMPLE_MODE: on, EXAMPLE_TOKEN: abc123",
    );
    describe(&h.folder, "git", "GIT_SSH_COMMAND: ssh -i key");
    let (agent, _) = h.start_claude(None).await;
    env_card(&h).await;
    let (question, _) = env_question(&mut h, agent).await;
    let consent = question.consent.clone().expect("the card lays it out");
    assert_eq!(consent.title, TITLE);
    assert!(consent.changes.is_empty());
    let row = |name: &str| {
        consent
            .rows
            .iter()
            .find(|row| row.name == name)
            .unwrap_or_else(|| panic!("no {name}: {consent:#?}"))
    };
    assert_eq!(row("EXAMPLE_MODE").value, "on");
    assert_eq!(row("EXAMPLE_MODE").group, "example");
    assert!(!row("EXAMPLE_MODE").secret);
    assert_eq!(row("EXAMPLE_MODE").warning, None);
    assert!(row("EXAMPLE_TOKEN").secret);
    assert_eq!(row("GIT_SSH_COMMAND").group, "git");
    assert_eq!(
        row("GIT_SSH_COMMAND").warning.as_deref(),
        maxhq_workflow::env_risk("GIT_SSH_COMMAND")
    );
    assert!(!row("GIT_SSH_COMMAND").strong);
    let remember = consent.remember.expect("a folder's project can keep it");
    assert_eq!(remember.id, REMEMBER);
    assert!(
        remember.settings.starts_with(
            "Allow keeps this for the project. Change it later in Settings › Projects › "
        ),
        "{}",
        remember.settings
    );
    // The plain text says the same, with the secret hidden.
    assert!(
        question.text.contains("EXAMPLE_MODE=on"),
        "{}",
        question.text
    );
    assert!(question.text.contains("EXAMPLE_TOKEN=[hidden]"));
    assert!(!question.text.contains("abc123"));
    assert!(question
        .text
        .contains("GIT_SSH_COMMAND=ssh -i key (can run code"));
    let offered: Vec<&str> = question.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(offered, [ALLOW_LABEL, NOT_NOW_LABEL]);
}

#[tokio::test]
async fn allowing_restarts_the_agent_with_the_variables() {
    let mut h = Harness::new();
    describe(&h.folder, "example", "EXAMPLE_MODE: on");
    let (agent, _) = h.start_claude(None).await;
    let card = env_card(&h).await;
    assert_eq!(
        answer(&mut h, card.id, ALLOW_LABEL, false).await,
        AnswerOutcome::Accepted
    );
    eventually("the restart", || h.holders.sessions().len() == 2).await;
    let launch = h.holders.launch(h.holders.last());
    assert_eq!(launch_var(&launch, "EXAMPLE_MODE").as_deref(), Some("on"));
    assert_eq!(launch_var(&launch, "MAXHQ_AGENT"), Some(agent.to_string()));
    no_env_card(&h).await;
    let (_, answers) = env_question(&mut h, agent).await;
    assert_eq!(
        answers,
        Some(vec![QuestionAnswer {
            id: QUESTION.into(),
            values: vec![ALLOW_LABEL.into()],
        }])
    );
    // An Allow for this agent didn't keep it for the project.
    assert!(h.fakes.workspaces.allowed().is_empty());

    // It keeps them through a crash, without asking again.
    let next = h.crash(h.holders.last(), 3).await;
    let launch = h.holders.launch(next);
    assert_eq!(launch_var(&launch, "EXAMPLE_MODE").as_deref(), Some("on"));
    no_env_card(&h).await;
}

#[tokio::test]
async fn a_variable_that_replaces_what_maxhq_set_is_marked_strongest_and_still_goes_once_allowed() {
    let mut h = Harness::new();
    describe(&h.folder, "claude", "CLAUDE_CONFIG_DIR: /elsewhere");
    let (agent, session) = h.start_claude(None).await;
    assert_eq!(
        launch_var(&h.holders.launch(session), "CLAUDE_CONFIG_DIR").as_deref(),
        None
    );
    let card = env_card(&h).await;
    let (question, _) = env_question(&mut h, agent).await;
    let consent = question.consent.expect("the card lays it out");
    assert!(consent.rows[0].strong);
    assert!(
        consent.rows[0]
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("Claude sign-in")),
        "{consent:#?}"
    );
    assert!(
        consent.risk.contains("allow them only if"),
        "{}",
        consent.risk
    );

    assert_eq!(
        answer(&mut h, card.id, ALLOW_LABEL, false).await,
        AnswerOutcome::Accepted
    );
    eventually("the restart", || h.holders.sessions().len() == 2).await;
    let launch = h.holders.launch(h.holders.last());
    assert_eq!(
        launch_var(&launch, "CLAUDE_CONFIG_DIR").as_deref(),
        Some("/elsewhere")
    );
}

#[tokio::test]
async fn not_now_carries_on_without_them_and_doesnt_ask_that_agent_again() {
    let mut h = Harness::new();
    describe(&h.folder, "example", "EXAMPLE_MODE: on");
    let (agent, session) = h.start_claude(None).await;
    let card = env_card(&h).await;
    assert_eq!(
        answer(&mut h, card.id, NOT_NOW_LABEL, false).await,
        AnswerOutcome::Accepted
    );
    no_env_card(&h).await;
    assert_eq!(h.holders.sessions().len(), 1);
    let (_, answers) = env_question(&mut h, agent).await;
    assert_eq!(
        answers.map(|answers| answers[0].values.clone()),
        Some(vec![NOT_NOW_LABEL.to_owned()])
    );

    let next = h.crash(session, 2).await;
    assert_eq!(launch_var(&h.holders.launch(next), "EXAMPLE_MODE"), None);
    no_env_card(&h).await;

    let (other, _) = h.start_claude(None).await;
    assert_ne!(other, agent);
    assert_eq!(env_card(&h).await.agent, other);
}

#[tokio::test]
async fn remembering_keeps_the_answer_for_the_project() {
    let mut h = Harness::new();
    describe(&h.folder, "example", "EXAMPLE_MODE: on");
    let (agent, _) = h.start_claude(None).await;
    let card = env_card(&h).await;
    assert_eq!(
        answer(&mut h, card.id, ALLOW_LABEL, true).await,
        AnswerOutcome::Accepted
    );
    let project = project(&h, agent);
    let allowed = h.fakes.workspaces.allowed();
    assert_eq!(allowed.len(), 1);
    assert_eq!(allowed[0].0, project);
    let names: Vec<&str> = allowed[0]
        .1
        .vars
        .iter()
        .map(|var| var.name.as_str())
        .collect();
    assert_eq!(names, ["EXAMPLE_MODE"]);
    assert_eq!(
        allowed[0].1.vars[0].digest,
        maxhq_transfer::receive::sha256_hex(b"on")
    );
    eventually("the restart", || h.holders.sessions().len() == 2).await;
    let (_, answers) = env_question(&mut h, agent).await;
    assert_eq!(
        answers.map(|answers| answers[0].values.clone()),
        Some(vec![
            ALLOW_LABEL.to_owned(),
            "Remember for this project".to_owned()
        ])
    );

    // The project's settings now hold it, so the next agent just gets them.
    h.keep(settings(project, true, Some(allowed[0].1.clone())));
    let (_, session) = h.start_claude(None).await;
    let launch = h.holders.launch(session);
    assert_eq!(launch_var(&launch, "EXAMPLE_MODE").as_deref(), Some("on"));
    no_env_card(&h).await;
}

#[tokio::test]
async fn allowing_for_the_project_frees_the_other_agents_waiting_for_the_same() {
    let mut h = Harness::new();
    describe(&h.folder, "example", "EXAMPLE_MODE: on");
    h.start_claude(None).await;
    env_card(&h).await;
    h.start_claude(None).await;
    eventually("two cards", || env_cards(&h).len() == 2).await;
    let id = env_cards(&h)[0].id;
    assert_eq!(
        answer(&mut h, id, ALLOW_LABEL, true).await,
        AnswerOutcome::Accepted
    );
    eventually("both restarts", || h.holders.sessions().len() == 4).await;
    let launch = h.holders.launch(h.holders.last());
    assert_eq!(launch_var(&launch, "EXAMPLE_MODE").as_deref(), Some("on"));
    no_env_card(&h).await;
}

#[tokio::test]
async fn a_change_asks_again_and_says_what_changed() {
    let mut h = Harness::new();
    describe(&h.folder, "example", "EXAMPLE_MODE: on");
    let (agent, _) = h.start_claude(None).await;
    let card = env_card(&h).await;
    assert_eq!(
        answer(&mut h, card.id, ALLOW_LABEL, false).await,
        AnswerOutcome::Accepted
    );
    eventually("the restart", || h.holders.sessions().len() == 2).await;

    // The follower saw the descriptor change.
    let cwd = h.holders.launch(h.holders.last()).cwd;
    let var = |name: &str, value: &str| Var {
        workflow: "example".into(),
        name: name.into(),
        value: value.into(),
    };
    h.fakes.workflows.envs.put(
        &cwd,
        vec![var("EXAMPLE_MODE", "off"), var("EXAMPLE_NEW", "1")],
    );
    let next = h.crash(h.holders.last(), 3).await;
    let launch = h.holders.launch(next);
    assert_eq!(launch_var(&launch, "EXAMPLE_MODE"), None);
    assert_eq!(launch_var(&launch, "EXAMPLE_NEW"), None);
    let card = env_card(&h).await;
    assert_eq!(card.agent, agent);
    let (question, _) = env_question(&mut h, agent).await;
    let consent = question.consent.clone().expect("the card lays it out");
    assert_eq!(consent.changes, ["changed EXAMPLE_MODE", "new EXAMPLE_NEW"]);
    assert!(
        question.text.contains("changed EXAMPLE_MODE"),
        "{}",
        question.text
    );
}

#[tokio::test]
async fn a_busy_agent_gets_them_only_after_its_turn_ends() {
    let mut h = Harness::new();
    describe(&h.folder, "example", "EXAMPLE_MODE: on");
    let (agent, session) = h.start_claude(None).await;
    let card = env_card(&h).await;
    h.say(session, &busy(&h, agent, Activity::Busy)).await;
    assert_eq!(
        answer(&mut h, card.id, ALLOW_LABEL, false).await,
        AnswerOutcome::Accepted
    );
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.holders.sessions().len(), 1);
    assert_eq!(env_cards(&h), Vec::new());

    h.say(session, &busy(&h, agent, Activity::Idle)).await;
    eventually("the restart", || h.holders.sessions().len() == 2).await;
    let launch = h.holders.launch(h.holders.last());
    assert_eq!(launch_var(&launch, "EXAMPLE_MODE").as_deref(), Some("on"));
}

#[tokio::test]
async fn a_card_survives_a_maxd_restart_and_can_still_be_allowed() {
    let mut h = Harness::new();
    describe(&h.folder, "example", "EXAMPLE_MODE: on");
    let (_, session) = h.start_claude(None).await;
    let card = env_card(&h).await;

    h.restart().await;
    eventually("the card back", || env_cards(&h).len() == 1).await;
    assert_eq!(env_cards(&h)[0].id, card.id);
    eventually("the holder adopted", || {
        h.holders.connects(session).len() > 1
    })
    .await;

    assert_eq!(
        answer(&mut h, card.id, ALLOW_LABEL, false).await,
        AnswerOutcome::Accepted
    );
    eventually("the restart", || h.holders.sessions().len() == 2).await;
    let launch = h.holders.launch(h.holders.last());
    assert_eq!(launch_var(&launch, "EXAMPLE_MODE").as_deref(), Some("on"));
    no_env_card(&h).await;
}

#[tokio::test]
async fn a_restart_an_allow_asked_for_still_happens_after_a_maxd_restart() {
    let mut h = Harness::new();
    describe(&h.folder, "example", "EXAMPLE_MODE: on");
    let (agent, session) = h.start_claude(None).await;
    let card = env_card(&h).await;
    h.say(session, &busy(&h, agent, Activity::Busy)).await;
    assert_eq!(
        answer(&mut h, card.id, ALLOW_LABEL, false).await,
        AnswerOutcome::Accepted
    );

    h.restart().await;
    eventually("the holder adopted", || {
        h.holders.connects(session).len() > 1
    })
    .await;
    h.holders.say(session, &busy(&h, agent, Activity::Idle));
    eventually("the restart", || h.holders.sessions().len() == 2).await;
    let launch = h.holders.launch(h.holders.last());
    assert_eq!(launch_var(&launch, "EXAMPLE_MODE").as_deref(), Some("on"));
}

#[tokio::test]
async fn only_a_person_can_answer_and_only_with_what_the_card_offers() {
    let mut h = Harness::new();
    describe(&h.folder, "example", "EXAMPLE_MODE: on");
    h.start_claude(None).await;
    let card = env_card(&h).await;

    let mut tool = Inbox {
        client: h.sessions.tool_client(ClientId::derive("agent-tools")),
        kept: Vec::new(),
    };
    let key = IdempotencyKey::generate();
    tool.send(pick(key, card.id, ALLOW_LABEL, true));
    assert_eq!(tool.answered(key).await, AnswerOutcome::UnknownEvent);
    assert_eq!(
        answer(&mut h, card.id, "Trust", false).await,
        AnswerOutcome::UnknownEvent
    );
    let key = IdempotencyKey::generate();
    h.phone.send(ClientMessage::Answer {
        idempotency_key: key,
        event: card.id,
        answer: AttentionAnswer::Option {
            id: ALLOW_LABEL.into(),
        },
        refs: Vec::new(),
    });
    assert_eq!(h.phone.answered(key).await, AnswerOutcome::UnknownEvent);
    assert_eq!(h.holders.sessions().len(), 1);
    assert_eq!(env_cards(&h).len(), 1);
    assert!(h.fakes.workspaces.allowed().is_empty());

    assert_eq!(
        answer(&mut h, card.id, ALLOW_LABEL, false).await,
        AnswerOutcome::Accepted
    );
}

#[tokio::test]
async fn a_project_with_the_variables_off_starts_without_them_and_asks_nothing() {
    let mut h = Harness::new();
    describe(&h.folder, "example", "EXAMPLE_MODE: on");
    let (agent, _) = h.start_claude(None).await;
    env_card(&h).await;
    let project = project(&h, agent);
    h.keep(settings(project, false, None));

    let (agent, session) = h.start_claude(None).await;
    let launch = h.holders.launch(session);
    assert_eq!(launch_var(&launch, "MAXHQ_AGENT"), Some(agent.to_string()));
    assert_eq!(
        launch_var(&launch, "EXAMPLE_MODE"),
        None,
        "the project turned them off"
    );
    tokio::time::sleep(QUIET).await;
    assert!(env_cards(&h).iter().all(|card| card.agent != agent));
}

#[tokio::test]
async fn deleting_the_agent_closes_its_card() {
    let mut h = Harness::new();
    describe(&h.folder, "example", "EXAMPLE_MODE: on");
    let (agent, session) = h.start_claude(None).await;
    env_card(&h).await;
    let gone = Say {
        gone: vec![agent],
        ..Say::default()
    };
    h.say(session, &gone).await;
    eventually("the card closing", || env_cards(&h).is_empty()).await;
}
