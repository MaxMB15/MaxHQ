// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The composer's lookups: the `@` path search in an agent's folder, the
//! `#` lookup in its repo, an agent's excerpt, the read tool that shares
//! its rendering, and what an agent the device can't find gets.

use maxhq_core::excerpt::file_name;
use maxhq_protocol::context::InlineRef;
use maxhq_protocol::find::{AgentExcerpt, GitItemSearch, PathKind, PathMatch};
use maxhq_protocol::processes::ProcessSignal;
use maxhq_protocol::timeline::AttachmentKind;

use super::*;
use crate::session::fake::ReadCall;

impl Harness {
    async fn find_paths(&mut self, agent: AgentId, query: &str) -> ActionOutcome {
        let query = query.to_owned();
        self.act(|key| ClientMessage::FindPaths { key, agent, query })
            .await
    }
}

#[tokio::test]
async fn a_path_search_walks_the_agents_folder() {
    let mut h = Harness::new();
    fs::create_dir_all(h.folder.join("src/domain")).unwrap();
    fs::create_dir_all(h.folder.join("target")).unwrap();
    fs::write(h.folder.join("src/main.rs"), "fn main() {}").unwrap();
    fs::write(h.folder.join("target/main"), "x").unwrap();
    fs::write(h.folder.join(".gitignore"), "target/\n").unwrap();
    let (agent, _) = h.start_claude(None).await;

    let found = h.find_paths(agent, "main.rs").await;
    assert_eq!(
        found,
        ActionOutcome::Paths {
            matches: vec![PathMatch {
                path: "src/main.rs".into(),
                kind: PathKind::File,
            }],
        }
    );
    let ActionOutcome::Paths { matches } = h.find_paths(agent, "dom").await else {
        panic!("the search answers with paths");
    };
    assert_eq!(matches[0].path, "src/domain");
    assert_eq!(matches[0].kind, PathKind::Folder);
}

impl Harness {
    async fn stage_found(&mut self, agent: AgentId, path: &str) -> ActionOutcome {
        let source = AttachmentSource::Found {
            agent,
            path: path.to_owned(),
        };
        self.act(|key| ClientMessage::StageAttachment { key, source })
            .await
    }
}

#[tokio::test]
async fn what_the_path_search_finds_stages_from_the_folder_it_searched() {
    let mut h = Harness::new();
    fs::create_dir_all(h.folder.join("src")).unwrap();
    fs::write(h.folder.join("src/main.rs"), "fn main() {}").unwrap();
    let outside = h.dir.path().join("secret.txt");
    fs::write(&outside, "x").unwrap();
    // Windows makes links only with Developer Mode or as an administrator,
    // and a name that isn't there is refused all the same.
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, h.folder.join("link.txt")).unwrap();
    let (agent, _) = h.start_claude(None).await;

    let ActionOutcome::Paths { matches } = h.find_paths(agent, "main.rs").await else {
        panic!("the search answers with paths");
    };
    let outcome = h.stage_found(agent, &matches[0].path).await;
    let ActionOutcome::Staged { attachment } = outcome else {
        panic!("the file wasn't staged: {outcome:?}");
    };
    assert_eq!(attachment.name, "main.rs");
    assert_eq!(
        fs::read_to_string(&attachment.path).unwrap(),
        "fn main() {}"
    );
    let outcome = h.stage_found(agent, "src").await;
    let ActionOutcome::Staged { attachment } = outcome else {
        panic!("the folder wasn't staged: {outcome:?}");
    };
    assert_eq!(attachment.kind, AttachmentKind::Folder);

    // Nothing outside the folder stages, by `..`, a link or a full path.
    let full = outside.display().to_string();
    for path in ["../secret.txt", "link.txt", full.as_str(), "gone.rs"] {
        let outcome = h.stage_found(agent, path).await;
        assert!(reason(&outcome).starts_with(path), "{path}: {outcome:?}");
    }
    let nobody = AgentId::generate();
    let outcome = h.stage_found(nobody, "src/main.rs").await;
    assert_eq!(reason(&outcome), "This device doesn't have that agent.");
}

#[tokio::test]
async fn a_file_the_agent_was_sent_stages_again_from_its_attachments() {
    let mut h = Harness::new();
    let (agent, _) = h.start_claude(None).await;
    let notes = h.dir.path().join("notes.txt");
    fs::write(&notes, "remember").unwrap();
    let path = notes.display().to_string();
    let outcome = h
        .act(|key| ClientMessage::StageAttachment {
            key,
            source: AttachmentSource::Path { path },
        })
        .await;
    let ActionOutcome::Staged { attachment } = outcome else {
        panic!("the path wasn't staged: {outcome:?}");
    };
    let send = |attachments: Vec<AttachmentId>| {
        move |key| ClientMessage::SendInput {
            key,
            agent,
            input: UserInput {
                text: "look".into(),
                attachments,
                refs: Vec::new(),
            },
            mode: SendMode::Queue,
        }
    };
    assert_eq!(h.act(send(vec![attachment.id])).await, ActionOutcome::Done);
    let attachments = h.dir.path().join("attachments");
    let sent = attachments
        .join(agent.to_string())
        .join(attachment.id.to_string())
        .join("notes.txt");
    assert!(sent.exists());

    // A paste of the sent message stages it from where the agent has it.
    let outcome = h.stage_found(agent, &sent.display().to_string()).await;
    let ActionOutcome::Staged { attachment: again } = outcome else {
        panic!("the sent file wasn't staged: {outcome:?}");
    };
    assert_ne!(again.id, attachment.id);
    assert_eq!(fs::read_to_string(&again.path).unwrap(), "remember");
    assert!(sent.exists(), "the agent keeps its copy");
    assert_eq!(h.act(send(vec![again.id])).await, ActionOutcome::Done);

    // Another agent's attachments and the staging folder stay out of reach.
    let other = attachments.join(AgentId::generate().to_string()).join("x");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("theirs.txt"), "x").unwrap();
    let staged = attachments.join("staged").join("y");
    fs::create_dir_all(&staged).unwrap();
    fs::write(staged.join("pending.txt"), "x").unwrap();
    for path in [other.join("theirs.txt"), staged.join("pending.txt")] {
        let path = path.display().to_string();
        let outcome = h.stage_found(agent, &path).await;
        assert!(reason(&outcome).starts_with(&path), "{path}: {outcome:?}");
    }
}

#[tokio::test]
async fn a_lookup_for_an_unknown_agent_is_refused_in_words() {
    let mut h = Harness::new();
    let nobody = AgentId::generate();
    let found = h.find_paths(nobody, "main").await;
    assert_eq!(reason(&found), "This device doesn't have that agent.");
    let listed = h
        .act(|key| ClientMessage::ListProcesses { key, agent: nobody })
        .await;
    assert_eq!(reason(&listed), "This device doesn't have that agent.");
    let stopped = h
        .act(|key| ClientMessage::SignalProcess {
            key,
            agent: nobody,
            pid: 4242,
            start: 1,
            signal: ProcessSignal::Term,
        })
        .await;
    assert_eq!(reason(&stopped), "This device doesn't have that agent.");
}

#[tokio::test]
async fn asking_again_under_the_same_key_looks_again() {
    let mut h = Harness::new();
    let (agent, _) = h.start_claude(None).await;
    let key = IdempotencyKey::generate();
    let ask = || ClientMessage::FindPaths {
        key,
        agent,
        query: "notes".into(),
    };
    h.phone.send(ask());
    assert_eq!(
        h.phone.result(key).await,
        ActionOutcome::Paths {
            matches: Vec::new()
        }
    );
    fs::write(h.folder.join("notes.txt"), "x").unwrap();
    h.phone.send(ask());
    let ActionOutcome::Paths { matches } = h.phone.result(key).await else {
        panic!("the search answers with paths");
    };
    assert_eq!(matches.len(), 1);
}

impl Harness {
    async fn find_git_items(&mut self, agent: AgentId, query: &str) -> GitItemSearch {
        self.find_git_items_in(agent, None, query).await
    }

    async fn find_git_items_in(
        &mut self,
        agent: AgentId,
        repo: Option<&str>,
        query: &str,
    ) -> GitItemSearch {
        let query = query.to_owned();
        let repo = repo.map(str::to_owned);
        let outcome = self
            .act(|key| ClientMessage::FindGitItems {
                key,
                agent,
                query,
                repo,
            })
            .await;
        let ActionOutcome::GitItems { search } = outcome else {
            panic!("the lookup answers with git items, not {outcome:?}");
        };
        search
    }
}

#[tokio::test]
async fn a_git_lookup_looks_in_the_repo_the_folders_remote_names() {
    let mut h = Harness::new();
    fs::create_dir_all(h.folder.join(".git")).unwrap();
    fs::write(
        h.folder.join(".git/config"),
        "[remote \"origin\"]\n\turl = git@github.com:owner/repo.git\n",
    )
    .unwrap();
    let (agent, _) = h.start_claude(None).await;
    let search = h.find_git_items(agent, "#12").await;
    assert_eq!(search.repo.as_deref(), Some("owner/repo"));
    // The tests' maxd holds no GitHub App.
    assert_eq!(
        search.unavailable.as_deref(),
        Some("This device has no GitHub App. Set one up in Settings.")
    );
}

#[tokio::test]
async fn a_git_lookup_looks_in_another_repo_when_asked() {
    let mut h = Harness::new();
    fs::create_dir_all(h.folder.join(".git")).unwrap();
    fs::write(
        h.folder.join(".git/config"),
        "[remote \"origin\"]\n\turl = git@github.com:owner/repo.git\n",
    )
    .unwrap();
    let (agent, _) = h.start_claude(None).await;
    let search = h.find_git_items_in(agent, Some("someone/else"), "7").await;
    assert_eq!(search.repo.as_deref(), Some("someone/else"));
    // It reads with what the device has, and the tests' maxd has no App.
    assert_eq!(
        search.unavailable.as_deref(),
        Some("This device has no GitHub App. Set one up in Settings.")
    );

    // An agent this device doesn't have is refused, whatever the repo.
    let unknown = AgentId::derive("nobody");
    let outcome = h
        .act(|key| ClientMessage::FindGitItems {
            key,
            agent: unknown,
            query: "7".into(),
            repo: Some("someone/else".into()),
        })
        .await;
    assert!(
        matches!(outcome, ActionOutcome::Refused { .. }),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_git_lookup_says_when_theres_no_github_repo() {
    let mut h = Harness::new();
    let (agent, _) = h.start_claude(None).await;
    let search = h.find_git_items(agent, "").await;
    assert_eq!(search.repo, None);
    assert_eq!(
        search.unavailable.as_deref(),
        Some("The agent's folder has no GitHub remote.")
    );

    fs::create_dir_all(h.folder.join(".git")).unwrap();
    fs::write(
        h.folder.join(".git/config"),
        "[remote \"origin\"]\n\turl = https://gitlab.example/owner/repo.git\n",
    )
    .unwrap();
    let search = h.find_git_items(agent, "").await;
    assert_eq!(search.repo, None);
    assert_eq!(
        search.unavailable.as_deref(),
        Some("The agent's repo isn't on GitHub.")
    );
}

/// A message you sent, with the context in it.
fn request(agent: AgentId, id: &str, text: &str, refs: Vec<InlineRef>) -> TimelineItem {
    let body = ItemBody::UserMessage {
        text: text.into(),
        attachments: Vec::new(),
        refs,
        delivery: Delivery::Sent,
    };
    item(agent, id, ItemStatus::Completed, body)
}

/// An agent attached to a message.
fn attached(agent: AgentId, title: &str) -> InlineRef {
    InlineRef::Agent {
        agent,
        title: title.into(),
        vendor: Vendor::Claude,
        device: device().id,
        excerpt: None,
    }
}

/// A read tool call `by` makes that names `agent`: of its first page, or
/// with `item` of that item whole.
fn read(key: &str, by: AgentId, agent: &str, item: Option<&str>) -> ReadCall {
    (
        key.into(),
        by,
        agent.into(),
        None,
        item.map(Into::into),
        None,
        None,
    )
}

/// A read tool call `by` makes of `item` of `agent` and `count` messages
/// on each side of it.
fn around(key: &str, by: AgentId, agent: &str, item: &str, count: usize) -> ReadCall {
    (
        key.into(),
        by,
        agent.into(),
        None,
        Some(item.into()),
        Some(count),
        None,
    )
}

/// A read tool call `by` makes of the quote of `item` of `agent` that
/// starts at `position`, in its place.
fn at(key: &str, by: AgentId, agent: &str, item: &str, position: u32) -> ReadCall {
    (
        key.into(),
        by,
        agent.into(),
        None,
        Some(item.into()),
        None,
        Some(position),
    )
}

/// A passage of an agent's item quoted in a message.
fn quoted(agent: AgentId, title: &str, item: &str) -> InlineRef {
    InlineRef::Quote {
        agent,
        agent_title: title.into(),
        item: ItemId(item.into()),
        text: "never expires".into(),
        position: 0,
        source: QuoteSource::Reply,
        from: None,
    }
}

impl Harness {
    async fn excerpt(&mut self, agent: AgentId, budget: u32) -> ActionOutcome {
        self.act(|key| ClientMessage::ExcerptAgent { key, agent, budget })
            .await
    }

    /// What a read tool call answered, once it's back.
    async fn read_back(&self, key: &str) -> String {
        let prefix = format!("tool {key} read ");
        eventually("the read", || {
            self.vendors
                .log()
                .iter()
                .any(|entry| entry.starts_with(&prefix))
        })
        .await;
        let log = self.vendors.log();
        let entry = log.iter().find(|entry| entry.starts_with(&prefix));
        entry.unwrap()[prefix.len()..].to_owned()
    }

    /// An outside session with a request and a reply in its history.
    pub(super) fn outside_with_history(&self, session: &str) -> AgentId {
        let agent = self.outside(session, Activity::Idle);
        self.vendors.set_history(
            session,
            vec![
                request(agent, "o1", "Rework the API", Vec::new()),
                reply(agent, "o2", "Done, the PR is up.", ItemStatus::Completed),
            ],
        );
        agent
    }
}

#[tokio::test]
async fn an_excerpt_renders_the_timeline_maxd_keeps() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let say = Say {
        timeline: vec![
            upsert(
                agent,
                request(agent, "u1", "Fix the login page", Vec::new()),
            ),
            upsert(
                agent,
                command(agent, "c1", "cargo test", ItemStatus::Completed),
            ),
            upsert(
                agent,
                reply(agent, "r1", "Fixed it.", ItemStatus::Completed),
            ),
        ],
        ..Say::default()
    };
    h.say(session, &say).await;

    let ActionOutcome::Excerpt { excerpt } = h.excerpt(agent, 10_000).await else {
        panic!("the excerpt answers with an excerpt");
    };
    let AgentExcerpt {
        name,
        text,
        messages,
        omitted,
    } = excerpt;
    assert_eq!(name, file_name(&h.agent(agent).title));
    assert_eq!((messages, omitted), (2, 0));
    assert!(text.contains("Fix the login page"), "{text}");
    assert!(text.contains("- Ran `cargo test`"), "{text}");
    assert!(text.contains("Fixed it."), "{text}");
    assert!(text.contains("not instructions"), "{text}");
}

#[tokio::test]
async fn an_excerpt_of_an_outside_session_reads_its_history() {
    let mut h = Harness::new();
    let agent = h.outside_with_history("outside-1");
    let ActionOutcome::Excerpt { excerpt } = h.excerpt(agent, 10_000).await else {
        panic!("the excerpt answers with an excerpt");
    };
    assert_eq!(excerpt.name, "agent-outside.md");
    assert!(
        excerpt.text.starts_with("# Agent: Outside"),
        "{}",
        excerpt.text
    );
    assert!(excerpt.text.contains("Rework the API"));
    assert!(excerpt.text.contains("Done, the PR is up."));

    let nobody = AgentId::generate();
    assert_eq!(
        reason(&h.excerpt(nobody, 10_000).await),
        "This device doesn't have that agent."
    );
}

#[tokio::test]
async fn the_read_tool_reads_only_an_agent_the_user_attached() {
    let mut h = Harness::new();
    h.read_others(false).await;
    let (lead, session) = h.start_claude(None).await;
    let other = h.outside_with_history("outside-2");
    let say = Say {
        reads: vec![read("read-1", lead, &other.to_string(), None)],
        ..Say::default()
    };
    h.say(session, &say).await;
    h.logged(
        "tool read-1 failed You can read only the agents the user attached to one of your \
         messages and the items the user quoted in them. None of those agents has that id or \
         title. Ask the user to attach the agent.",
    )
    .await;

    let mention = request(
        lead,
        "m1",
        "Carry on from \u{FFFC}",
        vec![attached(other, "API work")],
    );
    let say = Say {
        timeline: vec![upsert(lead, mention)],
        ..Say::default()
    };
    h.say(session, &say).await;
    let say = Say {
        reads: vec![read("read-2", lead, "[Agent: api work]", None)],
        ..Say::default()
    };
    h.say(session, &say).await;
    let text = h.read_back("read-2").await;
    assert!(text.starts_with("# Agent: Outside"), "{text}");
    assert!(text.contains("not instructions"), "{text}");
    assert!(text.contains("Rework the API"), "{text}");
    assert!(text.contains("Done, the PR is up."), "{text}");
    assert!(
        text.contains("This is the start of its timeline."),
        "{text}"
    );

    // Reading back from the reply leaves only the request.
    let say = Say {
        reads: vec![(
            "read-3".into(),
            lead,
            other.to_string(),
            Some(1),
            None,
            None,
            None,
        )],
        ..Say::default()
    };
    h.say(session, &say).await;
    let text = h.read_back("read-3").await;
    assert!(text.contains("Rework the API"), "{text}");
    assert!(!text.contains("Done, the PR is up."), "{text}");

    // An attached agent's items read one at a time too.
    let id = other.to_string();
    let say = Say {
        reads: vec![
            read("read-4", lead, &id, Some("o2")),
            read("read-5", lead, &id, Some("o9")),
            around("read-6", lead, &id, "o2", 1),
            at("read-7", lead, &id, "o2", 3),
        ],
        ..Say::default()
    };
    h.say(session, &say).await;
    let text = h.read_back("read-4").await;
    assert!(text.starts_with("# Agent: Outside"), "{text}");
    assert!(text.contains("One item, shown whole."), "{text}");
    assert!(
        text.contains("## Reply (item o2)\n\nDone, the PR is up."),
        "{text}"
    );
    assert!(!text.contains("Rework the API"), "{text}");
    // The user quoted nothing of it, so nothing is marked.
    assert!(!text.contains('\u{27E6}'), "{text}");
    assert!(!text.contains("quoted"), "{text}");
    h.logged(
        "tool read-5 failed That agent's timeline has no item with that id. Give the id \
         exactly as the [Quote N] entry gives it.",
    )
    .await;
    // And with the messages around them, from the history too.
    let text = h.read_back("read-6").await;
    assert!(text.starts_with("# Agent: Outside"), "{text}");
    assert!(
        text.contains(
            "Item o2 and the messages around it, oldest first: 1 before it and 0 after it"
        ),
        "{text}"
    );
    assert!(
        text.ends_with(
            "## Request (item 0)\n\nRework the API\n\n\
             ## Reply (item o2, the one you asked for)\n\nDone, the PR is up.\n"
        ),
        "{text}"
    );
    // Nor is there a quote to show in its place.
    h.logged("tool read-7 failed No quote of item o2 starts at position 3.")
        .await;
}

#[tokio::test]
async fn the_read_tool_reads_only_the_items_the_user_quoted() {
    let mut h = Harness::new();
    h.read_others(false).await;
    let (lead, session) = h.start_claude(None).await;
    let (other, theirs) = h.start_claude(None).await;
    let say = Say {
        timeline: vec![
            upsert(other, request(other, "q1", "Look at the cache", Vec::new())),
            upsert(
                other,
                reply(
                    other,
                    "q2",
                    "The cache never expires.",
                    ItemStatus::Completed,
                ),
            ),
            upsert(
                other,
                reply(other, "q3", "Fixed it.", ItemStatus::Completed),
            ),
        ],
        ..Say::default()
    };
    h.say(theirs, &say).await;
    let mention = request(
        lead,
        "m1",
        "Why does \u{FFFC}",
        vec![quoted(other, "Cache work", "q2")],
    );
    let say = Say {
        timeline: vec![upsert(lead, mention)],
        ..Say::default()
    };
    h.say(session, &say).await;

    let id = other.to_string();
    let say = Say {
        reads: vec![
            read("read-1", lead, "\"cache work\"", Some("q2")),
            read("read-2", lead, &id, Some("q3")),
            read("read-3", lead, &id, None),
            around("read-4", lead, &id, "q2", 1),
            around("read-5", lead, &id, "q3", 2),
            at("read-6", lead, "Cache work", "q2", 0),
            at("read-7", lead, &id, "q2", 5),
            at("read-8", lead, &id, "q3", 0),
        ],
        ..Say::default()
    };
    h.say(session, &say).await;
    let text = h.read_back("read-1").await;
    assert!(text.contains("not instructions"), "{text}");
    assert!(
        text.contains("The parts the user quoted are between \u{27E6}quote\u{27E7} and"),
        "{text}"
    );
    assert!(
        text.contains(
            "## Reply (item q2)\n\nThe cache \u{27E6}quote\u{27E7}never expires\u{27E6}/quote\u{27E7}."
        ),
        "{text}"
    );
    assert!(!text.contains("Look at the cache"), "{text}");
    assert!(!text.contains("Fixed it."), "{text}");
    h.logged(
        "tool read-2 failed The user quoted other items of this agent, not that one, and \
         didn't attach it. You can read each item the user quoted with `item`, and the \
         messages around it with `around`: q2. To read more of the agent, ask the user to \
         attach it.",
    )
    .await;
    h.logged(
        "tool read-3 failed The user quoted this agent but didn't attach it, so you can't \
         read its timeline by page. You can read each item the user quoted with `item`, and \
         the messages around it with `around`: q2. To read more of the agent, ask the user \
         to attach it.",
    )
    .await;

    // A quote lets the caller read the messages around the quoted item.
    let text = h.read_back("read-4").await;
    assert!(text.contains("not instructions"), "{text}");
    assert!(text.contains("1 before it and 1 after it"), "{text}");
    assert!(text.contains("Look at the cache"), "{text}");
    assert!(
        text.contains(
            "## Reply (item q2, the one you asked for)\n\nThe cache \u{27E6}quote\u{27E7}never \
             expires\u{27E6}/quote\u{27E7}."
        ),
        "{text}"
    );
    assert!(text.contains("Fixed it."), "{text}");
    assert!(!text.contains("before: "), "{text}");
    // But not around an item the user didn't quote.
    h.logged(
        "tool read-5 failed The user quoted other items of this agent, not that one, and \
         didn't attach it. You can read each item the user quoted with `item`, and the \
         messages around it with `around`: q2. To read more of the agent, ask the user to \
         attach it.",
    )
    .await;

    // A quote's position shows it in its place, in the item it's from.
    let text = h.read_back("read-6").await;
    assert!(
        text.contains("The quote at position 0 in its place in item q2"),
        "{text}"
    );
    assert!(text.contains("not instructions"), "{text}");
    assert!(
        text.ends_with(
            "## Reply (item q2)\n\nThe cache \u{27E6}quote\u{27E7}never expires\u{27E6}/quote\u{27E7}.\n"
        ),
        "{text}"
    );
    // But only from a position a quote of the item starts at.
    h.logged("tool read-7 failed No quote of item q2 starts at position 5, only at 0.")
        .await;
    h.logged(
        "tool read-8 failed The user quoted other items of this agent, not that one, and \
         didn't attach it. You can read each item the user quoted with `item`, and the \
         messages around it with `around`: q2. To read more of the agent, ask the user to \
         attach it.",
    )
    .await;
}

#[tokio::test]
async fn a_sub_agent_reads_what_its_leads_user_attached() {
    let mut h = Harness::new();
    h.read_others(false).await;
    let (lead, session) = h.start_claude(None).await;
    let sub = Agent {
        id: AgentId::generate(),
        parent: Some(lead),
        title: "Helper".into(),
        ..h.reported(lead, Activity::Busy)
    };
    let one = h.outside_with_history("outside-3");
    let two = h.outside_with_history("outside-4");
    let mut waiting = request(lead, "m2", "And \u{FFFC}", vec![attached(two, "Later")]);
    if let ItemBody::UserMessage { delivery, .. } = &mut waiting.body {
        *delivery = Delivery::Queued;
    }
    let say = Say {
        agents: vec![h.reported(lead, Activity::Busy), sub.clone()],
        timeline: vec![
            upsert(
                lead,
                request(lead, "m1", "See \u{FFFC}", vec![attached(one, "Outside")]),
            ),
            upsert(lead, waiting),
        ],
        ..Say::default()
    };
    h.say(session, &say).await;
    let say = Say {
        reads: vec![
            read("read-1", sub.id, &one.to_string(), None),
            read("read-2", sub.id, &two.to_string(), None),
        ],
        ..Say::default()
    };
    h.say(session, &say).await;
    let text = h.read_back("read-1").await;
    assert!(text.contains("Rework the API"), "{text}");
    // A queued message hasn't reached the agent yet.
    eventually("the refusal", || {
        h.vendors
            .log()
            .iter()
            .any(|entry| entry.starts_with("tool read-2 failed You can read only"))
    })
    .await;
}
