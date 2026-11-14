// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Agents kept from a folder's own files until you trust them.

use maxhq_protocol::guest::{FolderScan, ScannedFile, ShownFile};
use maxhq_protocol::timeline::{Question, QuestionAnswer};

use super::restarts::{server, workspace};
use super::*;
use crate::folder_trust::{
    cursor_data_dir, fingerprint, only, scan_here, CURSOR_DATA_VAR, NOTED_LABEL, QUESTION,
    RESTRICTED_ARGS, RESTRICTED_LABEL, TRUST_LABEL,
};

const HOOKS: &str = r#"{"hooks":{"SessionStart":[]}}"#;
const CODEX_MCP: &str = "[mcp_servers.x]\ncommand = \"x\"\n";

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("a file has a folder")).expect("the folder is made");
    fs::write(path, text).expect("the file is written");
}

fn settings(folder: &Path) -> PathBuf {
    folder.join(".claude/settings.json")
}

fn codex_config(folder: &Path) -> PathBuf {
    folder.join(".codex/config.toml")
}

fn pick(key: IdempotencyKey, event: AttentionId, label: &str) -> ClientMessage {
    ClientMessage::Answer {
        idempotency_key: key,
        event,
        answer: AttentionAnswer::Questions {
            answers: vec![QuestionAnswer {
                id: QUESTION.into(),
                values: vec![label.into()],
            }],
        },
        refs: Vec::new(),
    }
}

/// A linked worktree of the repository at `root`, laid out as git does it.
fn worktree(root: &Path, at: &Path) {
    let admin = root.join(".git/worktrees/wt");
    fs::create_dir_all(at).expect("the worktree is made");
    fs::create_dir_all(&admin).expect("the admin folder is made");
    fs::write(at.join(".git"), format!("gitdir: {}\n", admin.display())).expect("it's written");
    fs::write(
        admin.join("gitdir"),
        format!("{}\n", at.join(".git").display()),
    )
    .expect("it's written");
    fs::write(admin.join("commondir"), "../..\n").expect("it's written");
}

/// A message the agent has, so its CLI has a transcript.
fn delivered(agent: AgentId, text: &str) -> Say {
    let body = ItemBody::UserMessage {
        text: text.into(),
        attachments: Vec::new(),
        refs: Vec::new(),
        delivery: Delivery::Sent,
    };
    Say {
        timeline: vec![upsert(
            agent,
            item(agent, "message", ItemStatus::Completed, body),
        )],
        ..Say::default()
    }
}

fn has_flags(holders: &FakeHolders, session: SessionId) -> bool {
    let launch = holders.launch(session);
    let args: Vec<String> = launch
        .args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let every = RESTRICTED_ARGS
        .iter()
        .all(|flag| args.iter().any(|a| a == flag));
    let any = RESTRICTED_ARGS
        .iter()
        .any(|flag| args.iter().any(|a| a == flag));
    assert_eq!(every, any, "only some of the flags are on: {args:?}");
    every
}

/// What a guest's scan of a folder with a hook answers.
fn hook_scan() -> FolderScan {
    FolderScan {
        key: "/home/agent/work/repo".into(),
        files: vec![ScannedFile {
            name: ".claude/settings.json".into(),
            digest: "abc".into(),
        }],
        shown: vec![ShownFile {
            name: ".claude/settings.json".into(),
            text: HOOKS.into(),
            cut: false,
        }],
        failed: false,
    }
}

impl Harness {
    fn restricted(&self, session: SessionId) -> bool {
        has_flags(&self.holders, session)
    }

    fn launched_with(&self, session: SessionId, flag: &str) -> bool {
        self.holders
            .launch(session)
            .args
            .iter()
            .any(|arg| arg.to_string_lossy() == flag)
    }

    /// The folder trust cards open now.
    fn cards(&self) -> Vec<AttentionEvent> {
        self.hub
            .snapshot()
            .entities
            .into_iter()
            .filter_map(|entity| match entity {
                Entity::Attention(event) if event.summary.starts_with("Trust this folder") => {
                    Some(event)
                }
                _ => None,
            })
            .collect()
    }

    /// Waits for the one card there should be.
    async fn card(&self) -> AttentionEvent {
        eventually("a card", || !self.cards().is_empty()).await;
        let cards = self.cards();
        assert_eq!(cards.len(), 1, "{cards:#?}");
        cards.into_iter().next().expect("there's one")
    }

    async fn no_card(&self) {
        tokio::time::sleep(QUIET).await;
        assert_eq!(self.cards(), Vec::new());
    }

    async fn answer_card(&mut self, event: AttentionId, label: &str) -> AnswerOutcome {
        let key = IdempotencyKey::generate();
        self.phone.send(pick(key, event, label));
        self.phone.answered(key).await
    }

    fn trusted_pairs(&self) -> usize {
        let scan = only(&scan_here(&self.folder, &[]), Vendor::Claude);
        usize::from(
            self.db
                .folder_trusted(&scan.key, &fingerprint(&scan.files))
                .expect("it reads"),
        )
    }

    /// The question the agent's timeline shows for its card.
    async fn card_question(&mut self, agent: AgentId) -> Question {
        let items = match self.open(agent, None, 200).await {
            TimelineUpdate::Reset { items, .. } => items,
            other => panic!("expected a reset, got {other:?}"),
        };
        items
            .iter()
            .rev()
            .find_map(|item| match &item.body {
                ItemBody::Question { questions, .. } => questions.first().cloned(),
                _ => None,
            })
            .expect("the timeline has the card's question")
    }
}

#[tokio::test]
async fn a_folder_with_no_run_files_starts_as_today_even_with_a_claude_md() {
    let mut h = Harness::new();
    write(&h.folder.join("CLAUDE.md"), "be kind");
    let (_, session) = h.start_claude(None).await;
    assert!(!h.restricted(session));
    h.no_card().await;
}

#[tokio::test]
async fn a_hook_file_gets_the_flags_and_one_card_and_a_restart_keeps_both() {
    let mut h = Harness::new();
    write(&settings(&h.folder), HOOKS);
    let (agent, session) = h.start_claude(None).await;
    assert!(h.restricted(session));
    let card = h.card().await;
    assert_eq!(card.agent, agent);
    assert_eq!(card.kind, AttentionKind::Question);

    let next = h.crash(session, 2).await;
    assert!(h.restricted(next));
    assert_eq!(h.card().await.id, card.id);
}

#[tokio::test]
async fn the_card_says_what_it_costs_and_what_would_run() {
    let mut h = Harness::new();
    write(&settings(&h.folder), HOOKS);
    let (agent, _) = h.start_claude(None).await;
    h.card().await;
    let question = h.card_question(agent).await;
    assert!(question.text.contains("CLAUDE.md"));
    assert!(question.text.contains(".claude/settings.json"));
    assert!(question.text.contains("SessionStart"));
}

#[tokio::test]
async fn trusting_stores_the_pair_and_restarts_with_resume_and_no_flags() {
    let mut h = Harness::new();
    write(&settings(&h.folder), HOOKS);
    let (agent, session) = h.start_claude(None).await;
    let card = h.card().await;
    h.say(session, &delivered(agent, "hello")).await;

    assert_eq!(
        h.answer_card(card.id, TRUST_LABEL).await,
        AnswerOutcome::Accepted
    );
    assert_eq!(h.trusted_pairs(), 1);
    eventually("the restart", || h.holders.sessions().len() == 2).await;
    let next = h.holders.last();
    assert!(!h.restricted(next));
    assert!(h.launched_with(next, "--resume"));
    h.no_card().await;
}

#[tokio::test]
async fn trusting_an_agent_that_had_no_message_starts_it_anew_since_it_has_no_transcript() {
    let mut h = Harness::new();
    write(&settings(&h.folder), HOOKS);
    let (_, _) = h.start_claude(None).await;
    let card = h.card().await;
    assert_eq!(
        h.answer_card(card.id, TRUST_LABEL).await,
        AnswerOutcome::Accepted
    );
    eventually("the restart", || h.holders.sessions().len() == 2).await;
    let next = h.holders.last();
    assert!(!h.restricted(next));
    assert!(h.launched_with(next, "--session-id"));
    assert!(!h.launched_with(next, "--resume"));
}

#[tokio::test]
async fn trusting_also_frees_other_agents_with_the_same_files() {
    let mut h = Harness::new();
    write(&settings(&h.folder), HOOKS);
    let (_, _) = h.start_claude(None).await;
    h.card().await;
    let (_, _) = h.start_claude(None).await;
    eventually("two cards", || h.cards().len() == 2).await;
    let id = h.cards()[0].id;
    assert_eq!(
        h.answer_card(id, TRUST_LABEL).await,
        AnswerOutcome::Accepted
    );
    eventually("both restarts", || h.holders.sessions().len() == 4).await;
    assert!(!h.restricted(h.holders.last()));
    h.no_card().await;
}

#[tokio::test]
async fn another_worktree_of_a_trusted_project_with_the_same_files_needs_no_answer() {
    let mut h = Harness::new();
    let root = fs::canonicalize(&h.folder).expect("it exists");
    fs::create_dir_all(root.join(".git")).expect("the folder is made");
    let other = root.parent().expect("it has one").join("other");
    worktree(&root, &other);
    write(&settings(&root), HOOKS);
    write(&settings(&other), HOOKS);

    let (_, _) = h.start_claude(None).await;
    let card = h.card().await;
    assert_eq!(
        h.answer_card(card.id, TRUST_LABEL).await,
        AnswerOutcome::Accepted
    );
    eventually("the restart", || h.holders.sessions().len() == 2).await;

    h.folder = other;
    let (_, session) = h.start_claude(None).await;
    assert!(!h.restricted(session));
    h.no_card().await;
}

#[tokio::test]
async fn changed_files_get_the_flags_and_a_card_naming_what_changed() {
    let mut h = Harness::new();
    write(&settings(&h.folder), HOOKS);
    let (agent, _) = h.start_claude(None).await;
    let card = h.card().await;
    assert_eq!(
        h.answer_card(card.id, TRUST_LABEL).await,
        AnswerOutcome::Accepted
    );
    eventually("the restart", || h.holders.sessions().len() == 2).await;
    h.no_card().await;

    write(&settings(&h.folder), r#"{"hooks":{"Changed":[]}}"#);
    let next = h.crash(h.holders.last(), 3).await;
    assert!(h.restricted(next));
    let card = h.card().await;
    assert_eq!(card.agent, agent);
    let question = h.card_question(agent).await;
    assert!(
        question.text.contains("changed .claude/settings.json"),
        "{}",
        question.text
    );
}

#[tokio::test]
async fn keeping_restricted_stays_restricted_without_asking_again_until_a_new_agent() {
    let mut h = Harness::new();
    write(&settings(&h.folder), HOOKS);
    let (agent, session) = h.start_claude(None).await;
    let card = h.card().await;
    assert_eq!(
        h.answer_card(card.id, RESTRICTED_LABEL).await,
        AnswerOutcome::Accepted
    );
    assert_eq!(h.trusted_pairs(), 0);
    assert_eq!(h.cards(), Vec::new());

    let next = h.crash(session, 2).await;
    assert!(h.restricted(next));
    h.no_card().await;

    let (other, _) = h.start_claude(None).await;
    assert_ne!(other, agent);
    assert_eq!(h.card().await.agent, other);
}

#[tokio::test]
async fn a_busy_agent_restarts_only_after_its_turn_ends() {
    let mut h = Harness::new();
    write(&settings(&h.folder), HOOKS);
    let (agent, session) = h.start_claude(None).await;
    let card = h.card().await;
    let busy = Say {
        agents: vec![h.reported(agent, Activity::Busy)],
        ..Say::default()
    };
    h.say(session, &busy).await;

    assert_eq!(
        h.answer_card(card.id, TRUST_LABEL).await,
        AnswerOutcome::Accepted
    );
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.holders.sessions().len(), 1);
    assert_eq!(h.cards(), Vec::new());

    let idle = Say {
        agents: vec![h.reported(agent, Activity::Idle)],
        ..Say::default()
    };
    h.say(session, &idle).await;
    eventually("the restart", || h.holders.sessions().len() == 2).await;
    assert!(!h.restricted(h.holders.last()));
}

#[tokio::test]
async fn a_card_survives_a_maxd_restart_and_can_still_be_trusted() {
    let mut h = Harness::new();
    write(&settings(&h.folder), HOOKS);
    let (_, session) = h.start_claude(None).await;
    let card = h.card().await;

    h.restart().await;
    eventually("the card back", || h.cards().len() == 1).await;
    assert_eq!(h.cards()[0].id, card.id);
    eventually("the holder adopted", || {
        h.holders.connects(session).len() > 1
    })
    .await;

    assert_eq!(
        h.answer_card(card.id, TRUST_LABEL).await,
        AnswerOutcome::Accepted
    );
    assert_eq!(h.trusted_pairs(), 1);
    eventually("the restart", || h.holders.sessions().len() == 2).await;
    assert!(!h.restricted(h.holders.last()));
    h.no_card().await;
}

#[tokio::test]
async fn a_restart_trust_asked_for_still_happens_after_a_maxd_restart() {
    let mut h = Harness::new();
    write(&settings(&h.folder), HOOKS);
    let (agent, session) = h.start_claude(None).await;
    let card = h.card().await;
    let busy = Say {
        agents: vec![h.reported(agent, Activity::Busy)],
        ..Say::default()
    };
    h.say(session, &busy).await;
    assert_eq!(
        h.answer_card(card.id, TRUST_LABEL).await,
        AnswerOutcome::Accepted
    );

    h.restart().await;
    eventually("the holder adopted", || {
        h.holders.connects(session).len() > 1
    })
    .await;
    let idle = Say {
        agents: vec![h.reported(agent, Activity::Idle)],
        ..Say::default()
    };
    h.holders.say(session, &idle);
    eventually("the restart", || h.holders.sessions().len() == 2).await;
    assert!(!h.restricted(h.holders.last()));
    assert_eq!(h.cards(), Vec::new());
}

#[tokio::test]
async fn a_connection_with_no_person_behind_it_cant_answer() {
    let mut h = Harness::new();
    write(&settings(&h.folder), HOOKS);
    let (_, _) = h.start_claude(None).await;
    let card = h.card().await;

    let mut tool = Inbox {
        client: h.sessions.tool_client(ClientId::derive("agent-tools")),
        kept: Vec::new(),
    };
    let key = IdempotencyKey::generate();
    tool.send(pick(key, card.id, TRUST_LABEL));
    assert_eq!(tool.answered(key).await, AnswerOutcome::UnknownEvent);
    assert_eq!(h.trusted_pairs(), 0);
    assert_eq!(h.holders.sessions().len(), 1);
    assert_eq!(h.cards().len(), 1);

    assert_eq!(
        h.answer_card(card.id, TRUST_LABEL).await,
        AnswerOutcome::Accepted
    );
}

#[tokio::test]
async fn an_answer_the_card_doesnt_offer_leaves_it_open() {
    let mut h = Harness::new();
    write(&settings(&h.folder), HOOKS);
    let (_, _) = h.start_claude(None).await;
    let card = h.card().await;
    assert_eq!(
        h.answer_card(card.id, "allow").await,
        AnswerOutcome::UnknownEvent
    );
    assert_eq!(h.cards().len(), 1);
}

#[tokio::test]
async fn a_card_has_no_buttons_so_trusting_goes_through_the_file_list() {
    let mut h = Harness::new();
    write(&settings(&h.folder), HOOKS);
    h.start_claude(None).await;
    let card = h.card().await;
    assert_eq!(card.options, Vec::new());

    let key = IdempotencyKey::generate();
    h.phone.send(ClientMessage::Answer {
        idempotency_key: key,
        event: card.id,
        answer: AttentionAnswer::Option {
            id: TRUST_LABEL.into(),
        },
        refs: Vec::new(),
    });
    assert_eq!(h.phone.answered(key).await, AnswerOutcome::UnknownEvent);
    assert_eq!(h.trusted_pairs(), 0);
    assert_eq!(h.cards().len(), 1);
}

#[tokio::test]
async fn deleting_the_agent_closes_its_card() {
    let mut h = Harness::new();
    write(&settings(&h.folder), HOOKS);
    let (agent, session) = h.start_claude(None).await;
    h.card().await;
    let gone = Say {
        gone: vec![agent],
        ..Say::default()
    };
    h.say(session, &gone).await;
    eventually("the card closing", || h.cards().is_empty()).await;
}

#[tokio::test]
async fn a_container_with_no_run_files_starts_as_today() {
    let mut h = Harness::on(server());
    h.container(None);
    let (_, session) = h.start_boxed_claude(None).await;
    assert!(!has_flags(&h.guest(), session));
    h.no_card().await;
}

#[tokio::test]
async fn a_hook_in_a_container_gets_the_flags_and_a_card_under_the_workspace() {
    let mut h = Harness::on(server());
    let ws = h.container(None);
    h.fakes.places.scan(ws, Ok(hook_scan()));
    let (agent, session) = h.start_boxed_claude(None).await;
    assert!(has_flags(&h.guest(), session));
    assert!(h.holders.sessions().is_empty());
    let question = {
        h.card().await;
        h.card_question(agent).await
    };
    assert!(question.text.contains(".claude/settings.json"));
}

#[tokio::test]
async fn a_failed_read_in_a_container_is_restricted_and_cant_be_trusted() {
    let mut h = Harness::on(server());
    let ws = h.container(None);
    h.fakes.places.scan(ws, Err("the link is down".into()));
    let (agent, session) = h.start_boxed_claude(None).await;
    assert!(has_flags(&h.guest(), session));
    let card = h.card().await;
    let question = h.card_question(agent).await;
    let offered: Vec<&str> = question.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(offered, [RESTRICTED_LABEL]);
    assert_ne!(
        h.answer_card(card.id, TRUST_LABEL).await,
        AnswerOutcome::Accepted
    );
}

#[tokio::test]
async fn trusting_a_container_restarts_it_there_without_flags() {
    let mut h = Harness::on(server());
    let ws = h.container(None);
    h.fakes.places.scan(ws, Ok(hook_scan()));
    let (agent, session) = h.start_boxed_claude(None).await;
    let card = h.card().await;
    h.say_in(session, &delivered(agent, "hello")).await;
    assert_eq!(
        h.answer_card(card.id, TRUST_LABEL).await,
        AnswerOutcome::Accepted
    );

    let fingerprint = crate::folder_trust::fingerprint(&hook_scan().files);
    let key = format!("guest:{}:{}", workspace(), hook_scan().key);
    assert!(h.db.folder_trusted(&key, &fingerprint).expect("it reads"));
    // The same files in a folder here aren't trusted by that.
    assert!(!h
        .db
        .folder_trusted(&hook_scan().key, &fingerprint)
        .expect("it reads"));

    let guest = h.guest();
    eventually("the restart", || guest.sessions().len() == 2).await;
    let next = guest.last();
    assert!(!has_flags(&guest, next));
    assert!(guest
        .launch(next)
        .args
        .iter()
        .any(|arg| arg.to_string_lossy() == "--resume"));
    assert!(h.holders.sessions().is_empty());
    h.no_card().await;
}

#[tokio::test]
async fn a_codex_folder_with_run_files_loads_untrusted_and_gets_a_card() {
    let mut h = Harness::new();
    write(&codex_config(&h.folder), CODEX_MCP);
    let (agent, _) = h.start_codex("thread-1").await;
    h.logged("trust Some(false)").await;
    let card = h.card().await;
    assert_eq!(card.agent, agent);
    let question = h.card_question(agent).await;
    assert!(question.text.contains(".codex/config.toml"));
    assert!(question.text.contains("AGENTS.md"));
    assert!(question.text.contains("mcp_servers"));
}

#[tokio::test]
async fn a_codex_folder_with_only_instructions_and_claudes_files_loads_trusted() {
    let mut h = Harness::new();
    write(&h.folder.join("AGENTS.md"), "be kind");
    write(&settings(&h.folder), HOOKS);
    h.start_codex("thread-1").await;
    h.logged("trust Some(true)").await;
    h.no_card().await;
}

#[tokio::test]
async fn trusting_a_codex_card_loads_the_idle_thread_again_with_the_files() {
    let mut h = Harness::new();
    write(&codex_config(&h.folder), CODEX_MCP);
    let (agent, server) = h.start_codex("thread-1").await;
    let card = h.card().await;
    let idle = Say {
        agents: vec![h.reported(agent, Activity::Idle)],
        ..delivered(agent, "Ship it")
    };
    h.say(server, &idle).await;

    assert_eq!(
        h.answer_card(card.id, TRUST_LABEL).await,
        AnswerOutcome::Accepted
    );
    h.logged("adopt thread-1").await;
    let log = h.vendors.log();
    let at = |entry: &str| {
        log.iter()
            .rposition(|logged| logged == entry)
            .unwrap_or_else(|| panic!("{entry} isn't logged: {log:?}"))
    };
    assert!(at("stop") < at("trust Some(true)"));
    assert!(at("trust Some(true)") < at("adopt thread-1"));
    h.no_card().await;
    let scan = only(&scan_here(&h.folder, &[]), Vendor::Codex);
    assert!(h
        .db
        .folder_trusted(&scan.key, &fingerprint(&scan.files))
        .expect("it reads"));
    // What you trusted for Codex says nothing about Claude.
    let (_, session) = h.start_claude(None).await;
    assert!(!h.restricted(session));
}

#[tokio::test]
async fn a_codex_thread_with_no_turn_yet_waits_for_one_to_load_again() {
    let mut h = Harness::new();
    write(&codex_config(&h.folder), CODEX_MCP);
    let (agent, server) = h.start_codex("thread-1").await;
    let card = h.card().await;
    let idle = Say {
        agents: vec![h.reported(agent, Activity::Idle)],
        ..Say::default()
    };
    h.say(server, &idle).await;
    assert_eq!(
        h.answer_card(card.id, TRUST_LABEL).await,
        AnswerOutcome::Accepted
    );
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.count("stop"), 0);

    h.say(server, &delivered(agent, "Ship it")).await;
    h.logged("adopt thread-1").await;
}

impl Harness {
    /// The data folder a Cursor holder was started with, if one was set.
    fn cursor_data(&self, session: SessionId) -> Option<PathBuf> {
        self.holders
            .launch(session)
            .env
            .iter()
            .rev()
            .find(|(name, _)| name == CURSOR_DATA_VAR)
            .map(|(_, value)| PathBuf::from(value))
    }

    async fn start_cursor(&mut self) -> (AgentId, SessionId) {
        let spec = self.spec(Vendor::Cursor, None);
        let agent = started(
            self.act(|key| ClientMessage::StartAgent { key, spec })
                .await,
        );
        (agent, self.holders.last())
    }
}

/// Where `agent`, a restricted Cursor here, keeps its data.
fn restricted_data(agent: AgentId) -> PathBuf {
    let home = std::env::home_dir().expect("tests have a home");
    cursor_data_dir(&home.join(".cursor"), agent)
}

#[tokio::test]
async fn a_cursor_folder_with_run_files_starts_with_a_data_folder_of_its_own_until_trusted() {
    let mut h = Harness::new();
    h.cursor_account(LoginKind::ConfigDir);
    write(&h.folder.join(".cursor/mcp.json"), r#"{"mcpServers":{}}"#);
    write(
        &h.folder.join(".cursor/hooks.json"),
        r#"{"version":1,"hooks":{"stop":[{"command":"./scripts/stop.sh"}]}}"#,
    );
    let (agent, session) = h.start_cursor().await;
    assert_eq!(h.cursor_data(session), Some(restricted_data(agent)));
    // Cursor takes no flags for it, so the Claude ones stay off.
    assert!(!h.restricted(session));
    let card = h.card().await;
    assert_eq!(card.agent, agent);
    let question = h.card_question(agent).await;
    let offered: Vec<&str> = question.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(offered, [TRUST_LABEL, RESTRICTED_LABEL]);
    assert!(question.text.contains(".cursor/mcp.json"));
    assert!(question
        .text
        .contains(".cursor/hooks.json runs scripts/stop.sh"));

    // The old answer isn't on a new card.
    assert_eq!(
        h.answer_card(card.id, NOTED_LABEL).await,
        AnswerOutcome::UnknownEvent
    );
    assert_eq!(
        h.answer_card(card.id, TRUST_LABEL).await,
        AnswerOutcome::Accepted
    );
    eventually("the restart", || h.holders.sessions().len() == 2).await;
    assert_eq!(h.cursor_data(h.holders.last()), None);
    h.no_card().await;

    let (_, session) = h.start_cursor().await;
    assert_eq!(h.cursor_data(session), None);
    h.no_card().await;
}

#[tokio::test]
async fn a_cursor_kept_restricted_stays_so_and_a_new_one_asks_again() {
    let mut h = Harness::new();
    h.cursor_account(LoginKind::ConfigDir);
    write(&h.folder.join(".cursor/mcp.json"), r#"{"mcpServers":{}}"#);
    let (agent, session) = h.start_cursor().await;
    let card = h.card().await;
    assert_eq!(
        h.answer_card(card.id, RESTRICTED_LABEL).await,
        AnswerOutcome::Accepted
    );
    h.no_card().await;
    assert_eq!(h.holders.sessions(), [session]);
    assert_eq!(h.cursor_data(session), Some(restricted_data(agent)));

    let (next_agent, next) = h.start_cursor().await;
    assert_eq!(h.cursor_data(next), Some(restricted_data(next_agent)));
    h.card().await;
}

#[tokio::test]
async fn restricted_cursors_in_one_folder_dont_share_what_they_approve() {
    let mut h = Harness::new();
    h.cursor_account(LoginKind::ConfigDir);
    write(&h.folder.join(".cursor/mcp.json"), r#"{"mcpServers":{}}"#);
    let (one, first) = h.start_cursor().await;
    h.card().await;
    let (two, second) = h.start_cursor().await;
    assert_ne!(one, two);
    // An MCP server one approves goes in its data folder, which the other
    // doesn't read.
    let (first, second) = (h.cursor_data(first), h.cursor_data(second));
    assert_eq!(first, Some(restricted_data(one)));
    assert_eq!(second, Some(restricted_data(two)));
    let (first, second) = (first.unwrap_or_default(), second.unwrap_or_default());
    assert!(!first.starts_with(&second) && !second.starts_with(&first));
}

#[tokio::test]
async fn a_cursor_in_a_container_keeps_its_restricted_data_in_agents_home() {
    let mut h = Harness::on(server());
    let ws = h.container(None);
    let scan = FolderScan {
        files: vec![ScannedFile {
            name: ".cursor/mcp.json".into(),
            digest: "abc".into(),
        }],
        shown: Vec::new(),
        ..hook_scan()
    };
    h.fakes.places.scan(ws, Ok(scan));
    let account = h.cursor_account(LoginKind::CursorApiKey);
    h.fakes
        .credentials
        .set_key(account, "key_test-not-a-real-cursor-key");
    let agent = started(h.start_boxed(h.boxed(Vendor::Cursor, None)).await);
    let session = h.agent(agent).session.expect("the agent has a holder");
    let data = h
        .guest()
        .launch(session)
        .env
        .into_iter()
        .find(|(name, _)| name == CURSOR_DATA_VAR)
        .map(|(_, value)| PathBuf::from(value));
    let home = Path::new(crate::recipe::CURSOR_CONFIG);
    assert_eq!(data, Some(cursor_data_dir(home, agent)));
    h.card().await;
}

#[tokio::test]
async fn what_you_noted_for_cursor_before_isnt_trust() {
    let mut h = Harness::new();
    h.cursor_account(LoginKind::ConfigDir);
    write(&h.folder.join(".cursor/mcp.json"), r#"{"mcpServers":{}}"#);
    let scan = only(&scan_here(&h.folder, &[]), Vendor::Cursor);
    let old = scan.key.replacen("cursor-trust:", "cursor:", 1);
    h.db.trust_folder(
        &old,
        &fingerprint(&scan.files),
        Timestamp(1),
        ClientId::derive("phone"),
        &scan.files,
    )
    .expect("it's stored");
    let (agent, session) = h.start_cursor().await;
    assert_eq!(h.cursor_data(session), Some(restricted_data(agent)));
    h.card().await;
}
