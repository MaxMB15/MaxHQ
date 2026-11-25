// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Moving a held agent to another vendor: the same agent goes on in a new
//! session, which gets the conversation so far with its first message.

use maxhq_core::handoff::TOO_LITTLE;

use super::*;

/// Something you said.
fn said(agent: AgentId, id: &str, text: &str) -> TimelineItem {
    let body = ItemBody::UserMessage {
        text: text.into(),
        attachments: Vec::new(),
        refs: Vec::new(),
        delivery: Delivery::Sent,
    };
    item(agent, id, ItemStatus::Completed, body)
}

/// The marker where the agent moved, from what the phone holds.
fn marker(items: &[TimelineItem]) -> Vec<(Vendor, Vendor, ItemStatus)> {
    items
        .iter()
        .filter_map(|item| match &item.body {
            ItemBody::Handoff {
                from_vendor,
                to_vendor,
                ..
            } => Some((*from_vendor, *to_vendor, item.status)),
            _ => None,
        })
        .collect()
}

impl Harness {
    /// A Claude agent that did one turn and is idle, with its holder.
    pub(super) async fn claude_after_a_turn(&mut self) -> (AgentId, SessionId) {
        let (agent, session) = self.start_claude(Some("Build it")).await;
        let say = Say {
            agents: vec![self.reported(agent, Activity::Idle)],
            timeline: vec![
                upsert(agent, said(agent, "u1", "Build it")),
                upsert(agent, reply(agent, "a1", "Built it", ItemStatus::Completed)),
            ],
            ..Say::default()
        };
        self.say(session, &say).await;
        (agent, session)
    }

    /// Picks `model` under `account` for `agent`.
    async fn pick(&mut self, agent: AgentId, account: AccountId, model: &str) -> ActionOutcome {
        let model = model.to_owned();
        self.act(|key| ClientMessage::SetModel {
            key,
            agent,
            model,
            account: Some(account),
        })
        .await
    }

    /// Answers the thread start for `agent` that the Codex app-server got
    /// after log entry `from`, as thread `thread`. Returns the app-server.
    pub(super) async fn codex_thread(
        &self,
        agent: AgentId,
        from: usize,
        thread: &str,
    ) -> SessionId {
        eventually("the thread start", || {
            self.vendors.log()[from..]
                .iter()
                .any(|logged| logged.starts_with("start "))
        })
        .await;
        let log = self.vendors.log()[from..].to_vec();
        let key: IdempotencyKey = log
            .iter()
            .find_map(|logged| logged.strip_prefix("start "))
            .and_then(|key| key.parse().ok())
            .expect("the start names its key");
        let server = self.codex_server();
        let reported = Agent {
            id: agent,
            activity: Activity::Idle,
            ..self.codex_agent(thread, Activity::Idle)
        };
        let say = Say {
            agents: vec![reported],
            results: vec![(key, ActionOutcome::Started { agent })],
            ..Say::default()
        };
        self.say(server, &say).await;
        eventually("the thread taken in", || {
            self.agent(agent).vendor_session_id.as_deref() == Some(thread)
        })
        .await;
        server
    }

    /// The agent's timeline as a client opening it now sees it.
    pub(super) async fn items(&self, agent: AgentId) -> Vec<TimelineItem> {
        let mut viewer = Inbox::new(&self.sessions, ClientId::generate());
        viewer.send(ClientMessage::OpenTimeline {
            agent,
            since: None,
            limit: 100,
            lead: None,
        });
        reset(viewer.timeline(agent).await).1
    }
}

#[tokio::test]
async fn a_claude_agent_moves_to_codex_and_takes_its_conversation_along() {
    let mut h = Harness::new();
    let (agent, claude) = h.claude_after_a_turn().await;
    let codex = h.codex;
    let from = h.vendors.log().len();

    assert_eq!(h.pick(agent, codex, "gpt-5.6").await, ActionOutcome::Done);
    // The same agent, now on Codex, and its Claude CLI stops.
    let shown = h.agent(agent);
    assert_eq!(shown.vendor, Vendor::Codex);
    assert_eq!(shown.account, Some(codex));
    assert_eq!(shown.model.as_deref(), Some("gpt-5.6"));
    eventually("the Claude CLI stopping", || {
        h.holders.stops(claude) == [STOP_GRACE]
    })
    .await;

    // The marker sits after what came before, waiting for the handoff.
    let items = h.items(agent).await;
    assert_eq!(ids(&items)[..2], ["u1", "a1"]);
    assert_eq!(
        marker(&items),
        [(Vendor::Claude, Vendor::Codex, ItemStatus::Pending)]
    );

    // The agent was running, so its Codex thread starts now.
    let server = h.codex_thread(agent, from, "thread-1").await;
    assert_eq!(h.agent(agent).id, agent);
    assert!(h.sent(server).is_empty());

    // The first message carries the conversation in front of it.
    assert_eq!(h.send(agent, "Ship it").await, ActionOutcome::Done);
    let sent = h.sent(server);
    assert_eq!(sent.len(), 1, "{sent:#?}");
    assert!(sent[0].starts_with("<maxhq_handoff>\n"), "{}", sent[0]);
    assert!(sent[0].contains("full_thread_summary"), "{}", sent[0]);
    assert!(sent[0].contains("Built it"), "{}", sent[0]);
    assert!(sent[0].ends_with("User message:\nShip it"), "{}", sent[0]);
    let items = h.items(agent).await;
    assert_eq!(
        marker(&items),
        [(Vendor::Claude, Vendor::Codex, ItemStatus::Completed)]
    );
    assert_eq!(h.stored(agent).handoff, None);

    // Only the first one does.
    assert_eq!(h.send(agent, "And test it").await, ActionOutcome::Done);
    assert_eq!(h.sent(server)[1], "And test it");
}

#[tokio::test]
async fn moving_back_resumes_the_old_session_with_what_it_missed() {
    let mut h = Harness::new();
    let (agent, _) = h.claude_after_a_turn().await;
    let (uuid, _) = h.vendors.launches()[0];
    let (claude, codex) = (h.claude, h.codex);
    let from = h.vendors.log().len();
    assert_eq!(h.pick(agent, codex, "gpt-5.6").await, ActionOutcome::Done);
    let server = h.codex_thread(agent, from, "thread-1").await;
    assert_eq!(h.send(agent, "Ship it").await, ActionOutcome::Done);
    let say = Say {
        timeline: vec![upsert(
            agent,
            reply(agent, "c1", "Shipped it", ItemStatus::Completed),
        )],
        ..Say::default()
    };
    h.say(server, &say).await;

    let from = h.vendors.log().len();
    let holders = h.holders.sessions().len();
    assert_eq!(h.pick(agent, claude, "").await, ActionOutcome::Done);
    // Codex stops the thread, and Claude takes its own session up again.
    h.logged("stop").await;
    assert!(h.vendors.log()[from..].contains(&"stop".to_owned()));
    eventually("the Claude session resumed", || {
        h.vendors.launches().len() == 2
    })
    .await;
    assert_eq!(h.vendors.launches()[1], (uuid, true));
    eventually("its holder", || h.holders.sessions().len() > holders).await;
    let resumed = h.holders.last();
    let shown = h.agent(agent);
    assert_eq!(
        (shown.vendor, shown.account),
        (Vendor::Claude, Some(claude))
    );

    // It gets only what happened since it last saw the conversation.
    assert_eq!(h.send(agent, "Thanks").await, ActionOutcome::Done);
    eventually("the message", || !h.sent(resumed).is_empty()).await;
    let sent = h.sent(resumed);
    assert_eq!(sent.len(), 1, "{sent:#?}");
    assert!(
        sent[0].contains("delta_since_target_last_seen"),
        "{}",
        sent[0]
    );
    assert!(sent[0].contains("Shipped it"), "{}", sent[0]);
    assert!(!sent[0].contains("Built it"), "{}", sent[0]);
    assert!(sent[0].ends_with("User message:\nThanks"), "{}", sent[0]);
    let items = h.items(agent).await;
    assert_eq!(
        marker(&items),
        [
            (Vendor::Claude, Vendor::Codex, ItemStatus::Completed),
            (Vendor::Codex, Vendor::Claude, ItemStatus::Completed),
        ]
    );
}

#[tokio::test]
async fn a_codex_agent_moves_to_a_new_claude_session() {
    let mut h = Harness::new();
    let (agent, server) = h.start_codex("thread-1").await;
    let say = Say {
        agents: vec![h.reported(agent, Activity::Idle)],
        timeline: vec![
            upsert(agent, said(agent, "u1", "Ship it")),
            upsert(
                agent,
                reply(agent, "a1", "Shipped it", ItemStatus::Completed),
            ),
        ],
        ..Say::default()
    };
    h.say(server, &say).await;
    let claude = h.claude;
    let from = h.vendors.log().len();
    let holders = h.holders.sessions().len();

    assert_eq!(h.pick(agent, claude, "").await, ActionOutcome::Done);
    h.logged("stop").await;
    assert!(h.vendors.log()[from..].contains(&"stop".to_owned()));
    // A session of its own, named before it starts, with nothing to
    // resume.
    eventually("a Claude CLI", || h.vendors.launches().len() == 1).await;
    let (uuid, resume) = h.vendors.launches()[0];
    assert!(!resume);
    assert_eq!(h.stored(agent).vendor_session, uuid.to_string());
    eventually("its holder", || h.holders.sessions().len() > holders).await;
    let session = h.holders.last();
    assert_eq!(h.send(agent, "Test it").await, ActionOutcome::Done);
    eventually("the message", || !h.sent(session).is_empty()).await;
    let sent = h.sent(session);
    assert!(sent[0].contains("Shipped it"), "{}", sent[0]);
    assert!(sent[0].ends_with("User message:\nTest it"), "{}", sent[0]);
}

#[tokio::test]
async fn a_move_with_nothing_said_yet_carries_nothing() {
    let mut h = Harness::new();
    let (agent, _) = h.start_claude(None).await;
    let codex = h.codex;
    let from = h.vendors.log().len();
    assert_eq!(h.pick(agent, codex, "").await, ActionOutcome::Done);
    let items = h.items(agent).await;
    assert_eq!(
        marker(&items),
        [(Vendor::Claude, Vendor::Codex, ItemStatus::Completed)]
    );
    // The account's default model, as with a new agent.
    assert_eq!(h.agent(agent).model, None);
    let server = h.codex_thread(agent, from, "thread-1").await;
    assert_eq!(h.send(agent, "Hello").await, ActionOutcome::Done);
    assert_eq!(h.sent(server), ["Hello"]);
}

#[tokio::test]
async fn a_message_too_long_for_the_handoff_is_refused_and_the_handoff_waits() {
    let mut h = Harness::new();
    let (agent, _) = h.claude_after_a_turn().await;
    let codex = h.codex;
    let from = h.vendors.log().len();
    assert_eq!(h.pick(agent, codex, "").await, ActionOutcome::Done);
    let server = h.codex_thread(agent, from, "thread-1").await;

    // A message that fills the default window leaves no room.
    let long = "x".repeat(120_000);
    let outcome = h.send(agent, &long).await;
    assert_eq!(reason(&outcome), TOO_LITTLE);
    assert!(h.sent(server).is_empty());
    assert!(h.stored(agent).handoff.is_some());

    assert_eq!(h.send(agent, "Ship it").await, ActionOutcome::Done);
    assert!(h.sent(server)[0].contains("Built it"));
}

#[tokio::test]
async fn the_conversation_from_before_a_move_survives_a_restart() {
    let mut h = Harness::new();
    let (agent, _) = h.claude_after_a_turn().await;
    let codex = h.codex;
    let from = h.vendors.log().len();
    assert_eq!(h.pick(agent, codex, "").await, ActionOutcome::Done);
    h.codex_thread(agent, from, "thread-1").await;

    h.restart().await;
    eventually("the agent back", || {
        h.hub.get(&EntityKey::Agent(agent)).is_some()
    })
    .await;
    let items = h.items(agent).await;
    assert_eq!(ids(&items)[..2], ["u1", "a1"]);
    assert_eq!(
        marker(&items),
        [(Vendor::Claude, Vendor::Codex, ItemStatus::Pending)]
    );
    assert_eq!(h.agent(agent).vendor, Vendor::Codex);

    // The handoff still goes with the first message, to the thread it
    // adopts.
    assert_eq!(h.send(agent, "Ship it").await, ActionOutcome::Done);
    let server = h.codex_server();
    eventually("the message", || !h.sent(server).is_empty()).await;
    assert!(h.sent(server)[0].contains("Built it"));
    assert!(h.vendors.log().contains(&"adopt thread-1".to_owned()));
}

#[tokio::test]
async fn a_move_the_agent_cant_make_is_refused() {
    let mut h = Harness::new();
    let (agent, session) = h.claude_after_a_turn().await;
    let codex = h.codex;

    // An account on another device.
    let elsewhere = AccountId::derive("devbox codex");
    h.keep(Entity::Account(Account {
        id: elsewhere,
        device: DeviceId::derive("devbox"),
        vendor: Vendor::Codex,
        label: "Codex".into(),
        config_dir: String::new(),
        email: None,
        organization: None,
        plan: None,
        models: Vec::new(),
        usage: None,
        usage_failure: None,
        acp: None,
        login: LoginKind::ConfigDir,
        origin: None,
        model_server: None,
    }));
    let outcome = h.pick(agent, elsewhere, "").await;
    assert_eq!(reason(&outcome), "that account isn't on the agent's device");
    let outcome = h.pick(agent, AccountId::derive("gone"), "").await;
    assert_eq!(reason(&outcome), "that account isn't on the agent's device");

    // A sub-agent goes where its lead goes.
    let sub = Agent {
        id: AgentId::generate(),
        parent: Some(agent),
        title: "Helper".into(),
        ..h.reported(agent, Activity::Idle)
    };
    let say = Say {
        agents: vec![sub.clone()],
        ..Say::default()
    };
    h.say(session, &say).await;
    let outcome = h.pick(sub.id, codex, "").await;
    assert_eq!(reason(&outcome), "a sub-agent runs on its lead's vendor");

    // A turn in flight ends first.
    let say = Say {
        agents: vec![h.reported(agent, Activity::Busy)],
        ..Say::default()
    };
    h.say(session, &say).await;
    let outcome = h.pick(agent, codex, "").await;
    assert_eq!(
        reason(&outcome),
        "wait for the turn to end before moving the agent to Codex"
    );

    // None of it touched the agent.
    let shown = h.agent(agent);
    assert_eq!(shown.vendor, Vendor::Claude);
    assert!(h.holders.stops(session).is_empty());
    assert!(marker(&h.items(agent).await).is_empty());
}

#[tokio::test]
async fn a_model_under_the_agents_own_account_stays_a_model_change() {
    let mut h = Harness::new();
    let (agent, _) = h.claude_after_a_turn().await;
    let claude = h.claude;
    assert_eq!(h.pick(agent, claude, "opus").await, ActionOutcome::Done);
    h.logged("model opus").await;
    assert_eq!(h.agent(agent).vendor, Vendor::Claude);
    assert!(marker(&h.items(agent).await).is_empty());
}

/// A local model's ID on this Mac's own server.
const LOCAL_MODEL: &str = "qwen3.6-35b-a3b@ud-q4_k_xl";

/// Where the agent moved to and from Local, from what the phone holds.
fn local_marker(items: &[TimelineItem]) -> Vec<(bool, bool, ItemStatus)> {
    items
        .iter()
        .filter_map(|item| match &item.body {
            ItemBody::Handoff {
                from_vendor,
                to_vendor,
                from_local,
                to_local,
                ..
            } => {
                assert_eq!((*from_vendor, *to_vendor), (Vendor::Claude, Vendor::Claude));
                Some((*from_local, *to_local, item.status))
            }
            _ => None,
        })
        .collect()
}

impl Harness {
    /// A local model account: Claude Code on a model server, with a Claude
    /// config folder of its own.
    pub(super) fn local_account(&mut self) -> AccountId {
        let id = AccountId::derive("local");
        self.keep(Entity::Account(Account {
            id,
            device: self.host.id,
            vendor: Vendor::Claude,
            label: "This Mac".into(),
            config_dir: self.dir.path().join("local").display().to_string(),
            email: None,
            organization: None,
            plan: None,
            models: vec![ModelChoice {
                id: LOCAL_MODEL.into(),
                label: "Qwen3.6 35B A3B".into(),
                default: true,
                efforts: Vec::new(),
                default_effort: None,
                no_images: true,
            }],
            usage: None,
            usage_failure: None,
            acp: None,
            login: LoginKind::ModelServer,
            origin: None,
            model_server: Some(maxhq_protocol::ids::ModelServerId::derive("server")),
        }));
        self.fakes.credentials.set_model_server(
            id,
            crate::drive::ServerLogin {
                base_url: "http://127.0.0.1:40123".into(),
                token: maxhq_protocol::secret::Secret::new("local-key".into()),
                context: Some(65_536),
                model: Some(LOCAL_MODEL.into()),
                lean: true,
                cloud: false,
                max_outputs: Default::default(),
            },
        );
        id
    }
}

#[tokio::test]
async fn a_claude_agent_hands_off_to_local_and_back_since_neither_resumes_the_other() {
    let mut h = Harness::new();
    let (agent, claude_session) = h.claude_after_a_turn().await;
    let (first, _) = h.vendors.launches()[0];
    let (claude, local) = (h.claude, h.local_account());
    let holders = h.holders.sessions().len();

    // To Local: a new session in the local account's own config folder,
    // with the conversation handed over.
    assert_eq!(h.pick(agent, local, LOCAL_MODEL).await, ActionOutcome::Done);
    let shown = h.agent(agent);
    assert_eq!(
        (shown.vendor, shown.account, shown.model.as_deref()),
        (Vendor::Claude, Some(local), Some(LOCAL_MODEL))
    );
    eventually("the Claude CLI stopping", || {
        h.holders.stops(claude_session) == [STOP_GRACE]
    })
    .await;
    eventually("a local CLI", || h.vendors.launches().len() == 2).await;
    let (uuid, resume) = h.vendors.launches()[1];
    assert!(!resume);
    assert_ne!(uuid, first);
    assert_eq!(
        h.vendors.claude_logins(),
        [LoginKind::ConfigDir, LoginKind::ModelServer]
    );
    eventually("its holder", || h.holders.sessions().len() > holders).await;
    let on_local = h.holders.last();
    assert_eq!(h.send(agent, "Ship it").await, ActionOutcome::Done);
    eventually("the message", || !h.sent(on_local).is_empty()).await;
    let sent = h.sent(on_local);
    assert!(sent[0].starts_with("<maxhq_handoff>\n"), "{}", sent[0]);
    assert!(sent[0].contains("Built it"), "{}", sent[0]);
    assert!(sent[0].ends_with("User message:\nShip it"), "{}", sent[0]);
    let say = Say {
        timeline: vec![upsert(
            agent,
            reply(agent, "l1", "Shipped it", ItemStatus::Completed),
        )],
        ..Say::default()
    };
    h.say(on_local, &say).await;

    // And back: Claude resumes its own session with what it missed.
    let holders = h.holders.sessions().len();
    assert_eq!(h.pick(agent, claude, "").await, ActionOutcome::Done);
    eventually("the Claude session resumed", || {
        h.vendors.launches().len() == 3
    })
    .await;
    assert_eq!(h.vendors.launches()[2], (first, true));
    assert_eq!(h.vendors.claude_logins()[2], LoginKind::ConfigDir);
    eventually("its holder", || h.holders.sessions().len() > holders).await;
    let resumed = h.holders.last();
    assert_eq!(h.send(agent, "Thanks").await, ActionOutcome::Done);
    eventually("the message", || !h.sent(resumed).is_empty()).await;
    let sent = h.sent(resumed);
    assert!(sent[0].contains("Shipped it"), "{}", sent[0]);
    assert!(!sent[0].contains("Built it"), "{}", sent[0]);
    let items = h.items(agent).await;
    assert_eq!(
        local_marker(&items),
        [
            (false, true, ItemStatus::Completed),
            (true, false, ItemStatus::Completed),
        ]
    );
}
