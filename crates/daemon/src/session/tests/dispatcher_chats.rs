// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The dispatcher's chats: where a request goes, the moves between chats
//! and Keep here, finding a chat, saving one to Notes, and the list a
//! restart keeps.

use maxhq_core::handoff;
use maxhq_protocol::dispatch::{ChatAct, DispatcherChat};
use maxhq_protocol::ids::ProjectId;
use maxhq_protocol::model::DeviceSettingChange;
use serde_json::{json, Value};

use super::*;

/// What a chat you keep a moved request in is sent.
const KEEP_HERE: &str = "Keep this here. Answer my last message in this chat.";

/// What a chat you save to Notes is sent.
const SAVE_NOTE: &str = "Save this chat to Notes.";

/// Longer than the pause after which a request starts a new chat.
const PAUSE: Duration = Duration::from_secs(31 * 60);

impl Harness {
    /// Asks the dispatcher `text` as the Claude account, in a new chat
    /// when `new`.
    async fn ask_chat(&mut self, text: &str, new: bool) -> ActionOutcome {
        let account = self.claude;
        let text = text.to_owned();
        self.act(|key| ClientMessage::AskDispatcher {
            key,
            text,
            account,
            new,
            refs: Vec::new(),
        })
        .await
    }

    /// Sets the pause after which a request starts a new chat, 0 for never.
    async fn chat_gap(&mut self, minutes: u16) {
        let device = self.host.id;
        let outcome = self
            .act(|key| ClientMessage::UpdateDeviceSettings {
                key,
                device,
                changes: vec![DeviceSettingChange::ChatGapMinutes { value: minutes }],
            })
            .await;
        assert_eq!(outcome, ActionOutcome::Done);
    }

    /// Acts on a chat.
    async fn act_on(&mut self, chat: AgentId, act: ChatAct) -> ActionOutcome {
        self.act(|key| ClientMessage::ActOnChat {
            key,
            agent: chat,
            action: act,
        })
        .await
    }

    /// The chats clients see, by number.
    fn chats(&self) -> Vec<DispatcherChat> {
        let mut chats = self.hub.pick(|entity| match entity {
            Entity::DispatcherChat(chat) => Some(chat.clone()),
            _ => None,
        });
        chats.sort_by_key(|chat| chat.number);
        chats
    }

    /// The chat of `agent`, as clients see it.
    fn chat(&self, agent: AgentId) -> DispatcherChat {
        let chats = self.chats();
        let found = chats.into_iter().find(|chat| chat.agent == agent);
        found.unwrap_or_else(|| panic!("no chat for {agent}"))
    }

    /// Waits for `agent`'s CLI, as it runs now, to get a message that ends
    /// with `text`, and gives the message and its holder.
    async fn got(&self, agent: AgentId, text: &str) -> (String, SessionId) {
        let mut got = None;
        eventually(&format!("{text:?} at {agent}"), || {
            got = self.stored(agent).session.and_then(|session| {
                let mut sent = self.sent(session).into_iter();
                let found = sent.rfind(|sent| sent.ends_with(text))?;
                Some((found, session))
            });
            got.is_some()
        })
        .await;
        got.expect("found")
    }

    /// The project of the test folder.
    fn dispatch_project(&self) -> ProjectId {
        let workspace = self.hub.locate_folder(&self.folder);
        match self.hub.get(&EntityKey::Workspace(workspace)) {
            Some(Entity::Workspace(found)) => found.project.expect("a folder has a project"),
            other => panic!("no workspace: {other:?}"),
        }
    }
}

/// Names a chat as its dispatcher would.
fn name(title: &str, summary: &str) -> Value {
    json!({"title": title, "summary": summary})
}

#[tokio::test]
async fn a_request_soon_after_the_last_goes_to_the_chat_now() {
    let mut h = Harness::with_dispatcher().await;
    let (chat, session) = h.dispatch("Why is the login slow?").await;
    let shown = h.chat(chat);
    assert_eq!((shown.number, shown.title.as_str()), (1, ""));

    h.clock.advance(Duration::from_secs(29 * 60));
    assert_eq!(
        h.ask_chat("And the signup?", false).await,
        ActionOutcome::Done
    );
    eventually("the second request", || {
        h.sent(session).contains(&"And the signup?".to_owned())
    })
    .await;
    assert_eq!(h.chats().len(), 1);
    assert_eq!(h.dispatcher().agent, Some(chat));
}

#[tokio::test]
async fn a_shorter_pause_starts_a_chat_when_the_device_says_so() {
    let mut h = Harness::with_dispatcher().await;
    h.chat_gap(15).await;
    let (chat, session) = h.dispatch("Why is the login slow?").await;

    h.clock.advance(Duration::from_secs(14 * 60));
    assert_eq!(
        h.ask_chat("And the signup?", false).await,
        ActionOutcome::Done
    );
    eventually("the second request", || {
        h.sent(session).contains(&"And the signup?".to_owned())
    })
    .await;

    h.clock.advance(Duration::from_secs(16 * 60));
    let next = started(h.ask_chat("Plan the release notes", false).await);
    assert_ne!(next, chat);
    assert_eq!(h.dispatcher().agent, Some(next));
    h.got(next, "Plan the release notes").await;
    assert_eq!(h.chats().len(), 2);
}

#[tokio::test]
async fn with_never_only_a_new_conversation_starts_a_chat() {
    let mut h = Harness::with_dispatcher().await;
    h.chat_gap(0).await;
    let (chat, session) = h.dispatch("Why is the login slow?").await;

    h.clock.advance(Duration::from_secs(24 * 60 * 60));
    assert_eq!(
        h.ask_chat("And the signup?", false).await,
        ActionOutcome::Done
    );
    eventually("the second request", || {
        h.sent(session).contains(&"And the signup?".to_owned())
    })
    .await;
    assert_eq!(h.chats().len(), 1);
    assert_eq!(h.dispatcher().agent, Some(chat));

    let next = started(h.ask_chat("Plan the release notes", true).await);
    assert_ne!(next, chat);
    h.got(next, "Plan the release notes").await;
    assert_eq!(h.chats().len(), 2);
}

#[tokio::test]
async fn a_request_after_a_pause_starts_a_chat_that_lists_the_recent_ones() {
    let mut h = Harness::with_dispatcher().await;
    let (first, session) = h.dispatch("Why is the login slow?").await;
    let named = h
        .call(
            session,
            first,
            "n1",
            "name_chat",
            name("Slow login", "Why the login page takes ten seconds"),
        )
        .await;
    assert_eq!(named, Ok("Named the chat.".into()));
    assert_eq!(h.chat(first).title, "Slow login");

    h.clock.advance(PAUSE);
    let second = started(h.ask_chat("Plan the release notes", false).await);
    assert_ne!(second, first);
    assert_eq!(h.dispatcher().agent, Some(second));

    // Its first message lists the chat before, in a block the timeline
    // doesn't show.
    let (sent, _) = h.got(second, "Plan the release notes").await;
    assert!(handoff::is_wrapped(&sent), "{sent}");
    assert!(
        sent.contains("#1 Slow login, 31 minutes ago: Why the login page takes ten seconds"),
        "{sent}"
    );
    let numbers: Vec<(u32, AgentId)> = h.chats().iter().map(|c| (c.number, c.agent)).collect();
    assert_eq!(numbers, [(1, first), (2, second)]);

    // A new chat you ask for starts with no list.
    let third = started(h.ask_chat("Something else", true).await);
    let (sent, _) = h.got(third, "Something else").await;
    assert_eq!(sent, "Something else");
}

#[tokio::test]
async fn a_switch_resumes_an_earlier_chat_with_the_request() {
    let mut h = Harness::with_dispatcher().await;
    let (first, session) = h.dispatch("Why is the login slow?").await;
    h.call(
        session,
        first,
        "n1",
        "name_chat",
        name("Slow login", "Login speed"),
    )
    .await
    .expect("named");
    h.clock.advance(PAUSE);
    let second = started(h.ask_chat("Is the login fixed yet?", false).await);
    let (_, holder) = h.got(second, "Is the login fixed yet?").await;

    for (key, arguments, says) in [
        ("w1", json!({"chat": 9}), "There's no chat #9."),
        ("w2", json!({"chat": 2}), "That's this chat"),
    ] {
        let refused = h.call(holder, second, key, "switch_chat", arguments).await;
        assert!(refused.expect_err("refused").starts_with(says));
    }
    let moved = h
        .call(holder, second, "w3", "switch_chat", json!({"chat": 1}))
        .await
        .expect("moved");
    assert!(
        moved.starts_with("Moved the message to chat #1, \"Slow login\""),
        "{moved}"
    );

    // The first chat, stopped when the second started, takes it up.
    h.got(first, "Is the login fixed yet?").await;
    assert_eq!(h.dispatcher().agent, Some(first));
    assert_eq!(h.chat(second).moved_to, Some(first));

    // A chat that isn't the one now can't move another.
    let refused = h
        .call(holder, second, "w4", "switch_chat", json!({"chat": 1}))
        .await;
    assert_eq!(
        refused,
        Err("Only the chat the user writes in now can move a message.".into())
    );

    // Writing again settles the move, and the chat that held only the
    // moved request goes.
    assert_eq!(h.ask_chat("Thanks", false).await, ActionOutcome::Done);
    h.got(first, "Thanks").await;
    eventually("the empty chat goes", || h.chats().len() == 1).await;
    assert_eq!(h.chat(first).moved_to, None);
}

#[tokio::test]
async fn keep_here_moves_the_request_back() {
    let mut h = Harness::with_dispatcher().await;
    let (first, session) = h.dispatch("Why is the login slow?").await;
    let moved = h
        .call(
            session,
            first,
            "w1",
            "switch_chat",
            json!({"new": true, "title": "Login speed"}),
        )
        .await
        .expect("moved");
    assert!(
        moved.starts_with("Moved the message to chat #2, \"Login speed\""),
        "{moved}"
    );
    let second = h.dispatcher().agent.expect("a chat now");
    assert_ne!(second, first);
    h.got(second, "Why is the login slow?").await;
    assert_eq!(h.chat(first).moved_to, Some(second));
    assert_eq!(h.chat(second).title, "Login speed");

    assert_eq!(
        h.act_on(first, ChatAct::KeepHere).await,
        ActionOutcome::Done
    );
    h.got(first, KEEP_HERE).await;
    assert_eq!(h.dispatcher().agent, Some(first));
    assert_eq!(h.chat(first).moved_to, None);
    // The chat the move started goes with it.
    eventually("the moved-to chat goes", || h.chats().len() == 1).await;

    // A request you kept doesn't move again.
    let refused = h
        .call(session, first, "w2", "switch_chat", json!({"chat": 2}))
        .await;
    assert_eq!(
        refused,
        Err("The user kept the message in this chat, so answer it here.".into())
    );
    let outcome = h.act_on(first, ChatAct::KeepHere).await;
    assert_eq!(reason(&outcome), "Nothing moved out of that chat.");
}

#[tokio::test]
async fn find_chats_puts_a_title_match_before_a_match_in_the_text() {
    let mut h = Harness::with_dispatcher().await;
    let (first, session) = h.dispatch("Why does the form fail?").await;
    let answer = reply(
        first,
        "r1",
        "The login form sends an empty token.",
        ItemStatus::Completed,
    );
    let say = Say {
        timeline: vec![upsert(first, answer)],
        ..Say::default()
    };
    h.say(session, &say).await;
    h.call(
        session,
        first,
        "n1",
        "name_chat",
        name("Form errors", "An error on submit"),
    )
    .await
    .expect("named");
    let second = started(h.ask_chat("Help with the deploy", true).await);
    let (_, holder) = h.got(second, "Help with the deploy").await;
    h.call(
        holder,
        second,
        "n2",
        "name_chat",
        name("Login bug", "Deploys break it"),
    )
    .await
    .expect("named");
    let third = started(h.ask_chat("Something else", true).await);
    let (_, holder) = h.got(third, "Something else").await;

    let found = h
        .call(holder, third, "f1", "find_chats", json!({"query": "LOGIN"}))
        .await
        .expect("found");
    let found: Value = serde_json::from_str(&found).expect("JSON");
    let numbers: Vec<u64> = found["chats"]
        .as_array()
        .expect("chats")
        .iter()
        .map(|chat| chat["chat"].as_u64().expect("a number"))
        .collect();
    assert_eq!(numbers, [2, 1]);

    let none = h
        .call(
            holder,
            third,
            "f2",
            "find_chats",
            json!({"query": "kubernetes"}),
        )
        .await;
    assert_eq!(none, Ok("No earlier chat matches.".into()));
}

#[tokio::test]
async fn save_to_notes_asks_that_chat_for_a_note_in_its_project() {
    let mut h = Harness::with_dispatcher().await;
    let project = h.dispatch_project();
    let (first, session) = h.dispatch("Why is the login slow?").await;
    let named =
        json!({"title": "Slow login", "summary": "Login speed", "project": project.to_string()});
    h.call(session, first, "n1", "name_chat", named)
        .await
        .expect("named");
    assert_eq!(h.chat(first).project, Some(project));
    let second = started(h.ask_chat("Something else", true).await);
    let (_, other) = h.got(second, "Something else").await;

    assert_eq!(
        h.act_on(first, ChatAct::SaveNote).await,
        ActionOutcome::Done
    );
    let (_, holder) = h.got(first, SAVE_NOTE).await;
    assert_eq!(h.dispatcher().agent, Some(first));
    assert!(!h.sent(other).iter().any(|sent| sent == SAVE_NOTE));

    let note = json!({"title": "Slow login", "body": "The session check runs twice."});
    let made = h
        .call(holder, first, "c1", "create_note", note.clone())
        .await
        .expect("made");
    assert!(made.starts_with("Made the note "), "{made}");

    // A chat with no project asks which one first.
    let refused = h.call(other, second, "c2", "create_note", note).await;
    assert!(refused
        .expect_err("refused")
        .starts_with("This chat has no project"));
}

#[tokio::test]
async fn a_restart_keeps_the_chats_and_the_one_now() {
    let mut h = Harness::with_dispatcher().await;
    let (first, session) = h.dispatch("Why is the login slow?").await;
    h.call(
        session,
        first,
        "n1",
        "name_chat",
        name("Slow login", "Login speed"),
    )
    .await
    .expect("named");
    let second = started(h.ask_chat("Release notes", true).await);
    let (_, holder) = h.got(second, "Release notes").await;

    // Your title stays when the dispatcher names the chat.
    let renamed = ChatAct::Rename {
        title: " Notes for 2.0 \nmore".into(),
    };
    assert_eq!(h.act_on(second, renamed).await, ActionOutcome::Done);
    h.call(
        holder,
        second,
        "n2",
        "name_chat",
        name("Other", "What changed"),
    )
    .await
    .expect("named");
    let shown = h.chat(second);
    assert_eq!(
        (shown.title.as_str(), shown.summary.as_str()),
        ("Notes for 2.0", "What changed")
    );
    let archive = ChatAct::Archive { archived: true };
    assert_eq!(h.act_on(second, archive).await, ActionOutcome::Done);

    h.clock.advance(Duration::from_secs(60));
    assert_eq!(h.act_on(first, ChatAct::Pick).await, ActionOutcome::Done);
    assert_eq!(h.dispatcher().agent, Some(first));

    h.restart().await;
    eventually("the chats come back", || h.chats().len() == 2).await;
    let chats: Vec<(u32, AgentId, String, bool)> = h
        .chats()
        .into_iter()
        .map(|chat| (chat.number, chat.agent, chat.title, chat.archived))
        .collect();
    assert_eq!(
        chats,
        [
            (1, first, "Slow login".into(), false),
            (2, second, "Notes for 2.0".into(), true)
        ]
    );
    assert_eq!(h.dispatcher().agent, Some(first));
}

#[tokio::test]
async fn deleting_a_chats_agent_forgets_the_chat() {
    let mut h = Harness::with_dispatcher().await;
    let (first, _) = h.dispatch("Why is the login slow?").await;
    let outcome = h
        .act(|key| ClientMessage::DeleteAgent { key, agent: first })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    eventually("the chat goes", || h.chats().is_empty()).await;
    assert_eq!(h.dispatcher().agent, None);

    let outcome = h.act_on(first, ChatAct::Pick).await;
    assert_eq!(reason(&outcome), "That chat isn't here any more.");
}
