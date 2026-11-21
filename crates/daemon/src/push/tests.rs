// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Pushes against a local HTTP/2 fake of APNs, on a fake clock.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http2;
use hyper::service::service_fn;
use hyper::{Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use maxhq_protocol::attention::{AttentionEvent, AttentionKind, AttentionOption};
use maxhq_protocol::ids::{AgentId, AttentionId, PhoneId, WorkspaceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{
    Activity, Agent, AgentLink, Control, Vendor, Workspace, WorkspaceLocation,
};
use maxhq_protocol::phones::{Phone, PushEnvironment, PushKind, PushToken};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use super::seal::{open, public};
use super::*;
use crate::secrets::apns::test_p8;
use crate::secrets::testing::hub_for;
use crate::session::clock::FakeClock;
use crate::testing::TempDir;

/// What the fake got.
#[derive(Debug)]
struct Got {
    path: String,
    headers: HashMap<String, String>,
    body: Value,
}

/// A local APNs. Each token answers 200 unless told otherwise.
struct Fake {
    url: String,
    got: mpsc::UnboundedReceiver<Got>,
    answers: Arc<Mutex<HashMap<String, (u16, &'static str)>>>,
}

impl Fake {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (sent, got) = mpsc::unbounded_channel();
        let answers: Arc<Mutex<HashMap<String, (u16, &'static str)>>> = Arc::default();
        let held = answers.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let sent = sent.clone();
                let answers = held.clone();
                let service = service_fn(move |request: hyper::Request<Incoming>| {
                    let sent = sent.clone();
                    let answers = answers.clone();
                    async move {
                        let path = request.uri().path().to_owned();
                        let headers = request
                            .headers()
                            .iter()
                            .map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_owned()))
                            .collect();
                        let bytes = request.into_body().collect().await.unwrap().to_bytes();
                        let token = path.rsplit('/').next().unwrap_or_default().to_owned();
                        let (status, reason) = answers
                            .lock()
                            .unwrap()
                            .get(&token)
                            .copied()
                            .unwrap_or((200, ""));
                        let _ = sent.send(Got {
                            path,
                            headers,
                            body: serde_json::from_slice(&bytes).unwrap(),
                        });
                        let body = if reason.is_empty() {
                            String::new()
                        } else {
                            format!(r#"{{"reason":"{reason}"}}"#)
                        };
                        let mut response = Response::new(Full::new(Bytes::from(body)));
                        *response.status_mut() = StatusCode::from_u16(status).unwrap();
                        Ok::<_, Infallible>(response)
                    }
                });
                tokio::spawn(
                    http2::Builder::new(TokioExecutor::new())
                        .serve_connection(TokioIo::new(stream), service),
                );
            }
        });
        Self { url, got, answers }
    }

    fn answer(&self, token: &str, status: u16, reason: &'static str) {
        self.answers
            .lock()
            .unwrap()
            .insert(token.to_owned(), (status, reason));
    }

    fn take(&mut self) -> Vec<Got> {
        let mut got = Vec::new();
        while let Ok(one) = self.got.try_recv() {
            got.push(one);
        }
        got
    }
}

const TOKEN_A: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
const TOKEN_B: &str = "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2";
const TITLE_TEXT: &str = "Fix the merge";
const SUMMARY: &str = "Run cargo test --workspace?";

struct Harness {
    _temp: TempDir,
    clock: Arc<FakeClock>,
    hub: Hub,
    push: Push,
    fake: Fake,
    key: Arc<ApnsKey>,
}

fn phone_private(phone: PhoneId) -> [u8; 32] {
    let name = phone.to_string();
    let mut key = [0; 32];
    for (at, byte) in name.bytes().enumerate() {
        key[at % 32] ^= byte;
    }
    key
}

fn phone(name: &str) -> PhoneId {
    PhoneId::derive(name)
}

pub(crate) fn agent_id() -> AgentId {
    AgentId::derive("a")
}

pub(crate) fn attention_id() -> AttentionId {
    AttentionId::derive("prompt")
}

fn token(hex: &str, kinds: &[PushKind]) -> PushToken {
    PushToken {
        token: Secret::new(hex.into()),
        topic: "com.example.maxhq".into(),
        environment: PushEnvironment::Sandbox,
        kinds: kinds.to_vec(),
    }
}

const ALL: &[PushKind] = &[
    PushKind::NeedsInput,
    PushKind::Error,
    PushKind::Finished,
    PushKind::Limit,
];

impl Harness {
    async fn new() -> Self {
        let temp = TempDir::new();
        let hub = hub_for("devbox");
        let clock = Arc::new(FakeClock::new());
        let fake = Fake::start().await;
        let key = Arc::new(ApnsKey::parse("ABC123DEFG", "TEAM123456", &test_p8()).unwrap());
        let held = key.clone();
        let push = Push::open(
            hub.clone(),
            Folder::open(temp.path().join(DIR)).unwrap(),
            clock.clone(),
            Apns::at(fake.url.clone()),
            Arc::new(move || Some(held.clone())),
        );
        let device = hub.device().id;
        let mut entities = vec![Entity::Workspace(Workspace {
            id: WorkspaceId::derive("w"),
            project: None,
            device,
            location: WorkspaceLocation::Folder {
                path: "/src/maxhq".into(),
            },
            managed: None,
            clone: None,
        })];
        for name in ["phone-a", "phone-b"] {
            let id = phone(name);
            entities.push(Entity::Phone(Phone {
                id,
                device,
                name: name.into(),
                ssh_public: "ssh-ed25519 AAAA".into(),
                push_public: STANDARD.encode(public(&phone_private(id))),
                paired_at: Timestamp(1),
                push: Vec::new(),
                everything: false,
            }));
        }
        hub.apply(entities.into_iter().map(|value| Change::Upsert { value }));
        Self {
            _temp: temp,
            clock,
            hub,
            push,
            fake,
            key,
        }
    }

    fn watch(&self) -> Watch {
        let mut watch = Watch::new(self.push.clone());
        watch.reset(self.hub.snapshot().entities, true);
        watch
    }

    fn tokens(&self, kinds: &[PushKind]) {
        for (name, hex) in [("phone-a", TOKEN_A), ("phone-b", TOKEN_B)] {
            assert_eq!(
                self.push.set_token(phone(name), token(hex, kinds)),
                ActionOutcome::Done
            );
        }
    }

    async fn send_all(&self, watch: &mut Watch) -> Vec<Sent> {
        let mut sent = Vec::new();
        for out in watch.due() {
            sent.push(self.push.send(out).await);
        }
        sent
    }
}

fn agent(activity: Activity) -> Change {
    agent_titled(activity, TITLE_TEXT)
}

pub(crate) fn agent_titled(activity: Activity, title: &str) -> Change {
    Change::Upsert {
        value: Entity::Agent(Agent {
            id: agent_id(),
            workspace: WorkspaceId::derive("w"),
            parent: None,
            vendor: Vendor::Claude,
            account: None,
            model: None,
            permission: None,
            session: None,
            vendor_session_id: None,
            control: Control::Observed,
            pid: None,
            link: AgentLink::Live,
            title: title.into(),
            activity,
            context: None,
            started_at: Timestamp(1),
            last_activity_at: Timestamp(2),
            branch: None,
            inbox: Default::default(),
            origin: None,
            pulls: Vec::new(),
            taking_over: false,
            asleep: false,
            effort: None,
            running: None,
            moved: None,
            local: None,
        }),
    }
}

pub(crate) fn prompt(
    kind: AttentionKind,
    summary: &str,
    options: Vec<AttentionOption>,
) -> Vec<Change> {
    vec![
        Change::Upsert {
            value: Entity::Attention(AttentionEvent {
                id: attention_id(),
                agent: agent_id(),
                kind,
                summary: summary.into(),
                options,
                accepts_text: false,
                created_at: Timestamp(3),
                expires_at: None,
            }),
        },
        agent(Activity::NeedsInput {
            attention: attention_id(),
        }),
    ]
}

fn allow_deny() -> Vec<AttentionOption> {
    ["Allow", "Deny"]
        .into_iter()
        .map(|label| AttentionOption {
            id: label.to_lowercase(),
            label: label.into(),
            kind: None,
        })
        .collect()
}

/// Opens a push's sealed part with `phone`'s key.
fn opened(got: &Got, phone: PhoneId) -> Value {
    let sealed = STANDARD.decode(got.body["c"].as_str().unwrap()).unwrap();
    serde_json::from_slice(&open(&phone_private(phone), &sealed).unwrap()).unwrap()
}

#[tokio::test]
async fn push_waits_its_hold_then_goes_sealed_with_only_the_placeholder_in_the_clear() {
    let mut h = Harness::new().await;
    h.tokens(&[PushKind::NeedsInput]);
    let mut watch = h.watch();
    watch.observe(vec![agent(Activity::Busy)]);
    watch.observe(prompt(AttentionKind::Permission, SUMMARY, allow_deny()));

    h.clock.advance(HOLD - Duration::from_millis(1));
    assert!(watch.due().is_empty(), "still held");
    h.clock.advance(Duration::from_millis(1));
    assert_eq!(h.send_all(&mut watch).await, [Sent::Done, Sent::Done]);
    assert!(watch.due().is_empty(), "sent once");

    let mut got = h.fake.take();
    got.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].path, format!("/3/device/{TOKEN_A}"));
    assert_eq!(got[1].path, format!("/3/device/{TOKEN_B}"));
    let one = &got[0];
    assert_eq!(one.headers["apns-topic"], "com.example.maxhq");
    assert_eq!(one.headers["apns-push-type"], "alert");
    assert_eq!(
        one.headers["apns-collapse-id"],
        format!("needs-input:{}", attention_id())
    );
    assert!(one.headers["authorization"].starts_with("bearer "));
    assert_eq!(
        got[0].headers["authorization"], got[1].headers["authorization"],
        "one provider token for both"
    );

    // Only the placeholder in the clear.
    let mut clear = one.body.clone();
    let sealed = clear.as_object_mut().unwrap().remove("c").unwrap();
    assert_eq!(
        clear,
        serde_json::json!({
            "aps": {
                "alert": { "title": "MaxHQ", "body": "An agent needs you" },
                "mutable-content": 1,
                "thread-id": agent_id().to_string(),
                "category": "permission",
                "sound": "default",
                "interruption-level": "time-sensitive",
                "relevance-score": 1,
            },
            "e": attention_id().to_string(),
        })
    );
    // The headers too; the sealed part is random bytes.
    let text = format!("{clear} {:?}", one.headers);
    for private in [TITLE_TEXT, SUMMARY, "maxhq on devbox", "Allow"] {
        assert!(!text.contains(private), "{private} in the clear");
    }
    assert!(sealed.is_string());

    // Each phone opens its own, and not the other's.
    assert_eq!(
        opened(one, phone("phone-a")),
        serde_json::json!({
            "title": TITLE_TEXT,
            "subtitle": "devbox",
            "body": SUMMARY,
            "agent": agent_id(),
            "device": h.hub.device().id,
            "attention": attention_id(),
            "options": [
                { "id": "allow", "label": "Allow" },
                { "id": "deny", "label": "Deny" },
            ],
        })
    );
    let sealed = STANDARD.decode(one.body["c"].as_str().unwrap()).unwrap();
    assert!(open(&phone_private(phone("phone-b")), &sealed).is_err());
    opened(&got[1], phone("phone-b"));
}

#[tokio::test]
async fn push_is_dropped_when_the_prompt_clears_inside_the_hold() {
    let mut h = Harness::new().await;
    h.tokens(ALL);
    let mut watch = h.watch();
    watch.observe(vec![agent(Activity::Busy)]);
    watch.observe(prompt(AttentionKind::Permission, SUMMARY, allow_deny()));
    h.clock.advance(Duration::from_secs(3));
    // Answered on the Mac.
    watch.observe(vec![
        Change::Remove {
            key: EntityKey::Attention(attention_id()),
        },
        agent(Activity::Busy),
    ]);
    h.clock.advance(Duration::from_secs(10));
    assert!(watch.due().is_empty());
    assert!(watch.next().is_none());
    assert!(h.fake.take().is_empty());
}

#[tokio::test]
async fn push_skips_the_agent_a_phone_shows_until_presence_runs_out() {
    let mut h = Harness::new().await;
    h.tokens(&[PushKind::Finished]);
    let mut watch = h.watch();
    watch.observe(vec![agent(Activity::Busy)]);
    h.push.presence(phone("phone-a"), Some(agent_id()));
    h.push
        .presence(phone("phone-b"), Some(AgentId::derive("another")));
    watch.observe(vec![agent(Activity::DoneUnread)]);
    h.clock.advance(HOLD);
    assert_eq!(h.send_all(&mut watch).await, [Sent::Done]);
    let got = h.fake.take();
    assert_eq!(got[0].path, format!("/3/device/{TOKEN_B}"));
    assert_eq!(
        got[0].headers["apns-collapse-id"],
        format!("done:{}", agent_id())
    );
    assert!(got[0].body.get("e").is_none());
    let aps = &got[0].body["aps"];
    for field in ["category", "sound", "interruption-level", "relevance-score"] {
        assert!(aps.get(field).is_none(), "{field} on a finished turn");
    }

    // Presence counts for 45 seconds.
    h.clock.advance(PRESENCE - HOLD);
    watch.observe(vec![agent(Activity::Busy)]);
    watch.observe(vec![agent(Activity::DoneUnread)]);
    h.clock.advance(HOLD);
    assert_eq!(h.send_all(&mut watch).await, [Sent::Done, Sent::Done]);
    assert_eq!(h.fake.take().len(), 2);
}

#[tokio::test]
async fn push_follows_each_phones_switches() {
    let mut h = Harness::new().await;
    h.push
        .set_token(phone("phone-a"), token(TOKEN_A, &[PushKind::Limit]));
    h.push.set_token(phone("phone-b"), token(TOKEN_B, &[]));
    let mut watch = h.watch();
    watch.observe(vec![agent(Activity::Busy)]);
    watch.observe(prompt(AttentionKind::Question, SUMMARY, Vec::new()));
    watch.observe(vec![agent(Activity::DoneUnread)]);
    h.clock.advance(HOLD);
    assert!(watch.due().is_empty(), "neither wants prompts or turns");

    watch.observe(vec![agent(Activity::WaitingForLimitReset {
        resets_at: None,
    })]);
    h.clock.advance(HOLD);
    assert_eq!(h.send_all(&mut watch).await, [Sent::Done]);
    let got = h.fake.take();
    assert_eq!(got[0].path, format!("/3/device/{TOKEN_A}"));
    assert!(got[0].body["aps"].get("interruption-level").is_none());

    // An error prompt goes by the error switch.
    h.push
        .set_token(phone("phone-b"), token(TOKEN_B, &[PushKind::Error]));
    watch.observe(vec![agent(Activity::Busy)]);
    watch.observe(prompt(AttentionKind::Error, "It stopped", Vec::new()));
    h.clock.advance(HOLD);
    assert_eq!(h.send_all(&mut watch).await, [Sent::Done]);
    let got = h.fake.take();
    assert_eq!(got[0].path, format!("/3/device/{TOKEN_B}"));
    assert!(got[0].body["aps"].get("category").is_none());
    assert_eq!(got[0].body["aps"]["interruption-level"], "time-sensitive");
}

#[tokio::test]
async fn push_reminds_once_of_a_prompt_still_open() {
    let mut h = Harness::new().await;
    h.tokens(ALL);
    let mut watch = h.watch();
    watch.observe(vec![agent(Activity::Busy)]);
    watch.observe(prompt(AttentionKind::Permission, SUMMARY, allow_deny()));
    h.clock.advance(HOLD);
    assert_eq!(h.send_all(&mut watch).await, [Sent::Done, Sent::Done]);
    let first = h.fake.take();

    h.clock.advance(REMIND - Duration::from_millis(1));
    assert!(watch.due().is_empty(), "not yet");
    // Phone B opened the agent since.
    h.push.presence(phone("phone-b"), Some(agent_id()));
    h.clock.advance(Duration::from_millis(1));
    assert_eq!(h.send_all(&mut watch).await, [Sent::Done]);
    let again = h.fake.take();
    assert_eq!(again[0].path, format!("/3/device/{TOKEN_A}"));
    assert_eq!(
        again[0].headers["apns-collapse-id"], first[0].headers["apns-collapse-id"],
        "in place of the first"
    );
    assert_eq!(again[0].body["aps"], first[0].body["aps"]);

    // Only the one.
    assert!(watch.next().is_none());
    h.clock.advance(REMIND * 4);
    assert!(watch.due().is_empty());
}

#[tokio::test]
async fn push_reminds_of_nothing_answered_or_unpushed() {
    let mut h = Harness::new().await;
    h.tokens(ALL);
    let mut watch = h.watch();
    watch.observe(vec![agent(Activity::Busy)]);
    watch.observe(prompt(AttentionKind::Question, SUMMARY, Vec::new()));
    h.clock.advance(HOLD);
    assert_eq!(h.send_all(&mut watch).await.len(), 2);
    h.fake.take();
    // Answered before the reminder.
    h.clock.advance(Duration::from_secs(60));
    watch.observe(vec![
        Change::Remove {
            key: EntityKey::Attention(attention_id()),
        },
        agent(Activity::Busy),
    ]);
    assert!(watch.next().is_none());
    h.clock.advance(REMIND);
    assert!(watch.due().is_empty());

    // A turn's end gets none.
    watch.observe(vec![agent(Activity::DoneUnread)]);
    h.clock.advance(HOLD);
    assert_eq!(h.send_all(&mut watch).await.len(), 2);
    assert!(watch.next().is_none());

    // Nor a prompt that went to no phone, as one each phone shows.
    h.push.presence(phone("phone-a"), Some(agent_id()));
    h.push.presence(phone("phone-b"), Some(agent_id()));
    watch.observe(vec![agent(Activity::Busy)]);
    watch.observe(prompt(AttentionKind::Permission, SUMMARY, allow_deny()));
    h.clock.advance(HOLD);
    assert!(watch.due().is_empty());
    assert!(watch.next().is_none());
}

#[tokio::test]
async fn push_drops_a_token_apns_calls_gone() {
    let h = Harness::new().await;
    h.tokens(&[PushKind::Finished]);
    h.fake.answer(TOKEN_A, 410, "Unregistered");
    let mut watch = h.watch();
    watch.observe(vec![agent(Activity::Busy)]);
    watch.observe(vec![agent(Activity::DoneUnread)]);
    h.clock.advance(HOLD);
    let mut sent = h.send_all(&mut watch).await;
    sent.sort_by_key(|sent| *sent != Sent::Gone);
    assert_eq!(sent, [Sent::Gone, Sent::Done]);
    let state = h.push.lock();
    assert!(state.tokens.get(phone("phone-a")).is_none());
    assert!(state.tokens.get(phone("phone-b")).is_some());
    drop(state);

    // The file has only the one left.
    let again = Push::open(
        h.hub.clone(),
        Folder::open(h._temp.path().join(DIR)).unwrap(),
        h.clock.clone(),
        Apns::at(h.fake.url.clone()),
        Arc::new(|| None),
    );
    let state = again.lock();
    assert!(state.tokens.get(phone("phone-a")).is_none());
    assert!(state.tokens.get(phone("phone-b")).is_some());
}

#[tokio::test]
async fn push_makes_a_new_provider_token_once_apns_says_it_expired() {
    let mut h = Harness::new().await;
    h.push
        .set_token(phone("phone-a"), token(TOKEN_A, &[PushKind::Finished]));
    let first = h.key.token(h.clock.wall()).unwrap();
    h.fake.answer(TOKEN_A, 403, "ExpiredProviderToken");
    let mut watch = h.watch();
    watch.observe(vec![agent(Activity::Busy)]);
    watch.observe(vec![agent(Activity::DoneUnread)]);
    h.clock.advance(HOLD);
    h.send_all(&mut watch).await;
    let got = h.fake.take();
    assert_eq!(got.len(), 2, "tried again once");
    assert_eq!(
        got[0].headers["authorization"],
        format!("bearer {}", first.expose())
    );
    assert_ne!(
        got[1].headers["authorization"],
        got[0].headers["authorization"]
    );
}

#[tokio::test]
async fn push_stays_under_four_kilobytes() {
    let mut h = Harness::new().await;
    h.tokens(&[PushKind::NeedsInput]);
    let mut watch = h.watch();
    let long = "\"quoted\" é ".repeat(2_000);
    watch.observe(vec![agent_titled(Activity::Busy, &long)]);
    let options = (0..8)
        .map(|n| AttentionOption {
            id: format!("option-{n}"),
            label: "\u{1}".repeat(60),
            kind: None,
        })
        .collect();
    let mut changes = prompt(AttentionKind::Question, &long, options);
    changes[1] = agent_titled(
        Activity::NeedsInput {
            attention: attention_id(),
        },
        &long,
    );
    watch.observe(changes);
    h.clock.advance(HOLD);
    let outgoing = watch.due();
    assert_eq!(outgoing.len(), 2);
    for out in &outgoing {
        assert!(out.payload.len() < 4_096, "{} bytes", out.payload.len());
    }
    h.push.send(outgoing.into_iter().next().unwrap()).await;
    let got = h.fake.take();
    let sealed = opened(&got[0], phone("phone-a"));
    assert!(sealed["title"].as_str().unwrap().starts_with("\"quoted\""));
    assert_eq!(sealed["attention"], serde_json::json!(attention_id()));
}

#[tokio::test]
async fn push_skips_what_was_waiting_at_start_and_agents_on_other_devices() {
    let mut h = Harness::new().await;
    h.tokens(ALL);
    h.hub
        .apply(prompt(AttentionKind::Permission, SUMMARY, allow_deny()));
    let mut watch = h.watch();
    h.clock.advance(HOLD);
    assert!(watch.due().is_empty(), "already waiting at start");

    // A peer's agent is the peer's to push.
    watch.observe(vec![Change::Upsert {
        value: Entity::Workspace(Workspace {
            id: WorkspaceId::derive("w"),
            project: None,
            device: maxhq_protocol::ids::DeviceId::derive("elsewhere"),
            location: WorkspaceLocation::Folder {
                path: "/src/maxhq".into(),
            },
            managed: None,
            clone: None,
        }),
    }]);
    watch.observe(vec![agent(Activity::Busy)]);
    watch.observe(vec![agent(Activity::DoneUnread)]);
    h.clock.advance(HOLD);
    assert!(watch.due().is_empty());
    assert!(h.fake.take().is_empty());
}

#[tokio::test]
async fn push_refuses_tokens_that_arent_and_forgets_a_phone() {
    let h = Harness::new().await;
    let set = |hex: &str, topic: &str| {
        let mut push = token(hex, ALL);
        push.topic = topic.into();
        h.push.set_token(phone("phone-a"), push)
    };
    for outcome in [
        set("not hex at all, not hex at all", "com.example.maxhq"),
        set("abc", "com.example.maxhq"),
        set(&format!("{TOKEN_A}/../x"), "com.example.maxhq"),
        set(TOKEN_A, ""),
        set(TOKEN_A, "com.example.maxhq\nx"),
    ] {
        let ActionOutcome::Refused { reason } = outcome else {
            panic!("refused");
        };
        assert!(!reason.contains(TOKEN_A), "{reason}");
    }
    assert!(h.push.lock().tokens.get(phone("phone-a")).is_none());
    h.tokens(ALL);
    h.push.forget(phone("phone-a"));
    assert!(h.push.lock().tokens.get(phone("phone-a")).is_none());
    assert!(h.push.lock().tokens.get(phone("phone-b")).is_some());
}

#[tokio::test]
async fn push_runs_from_the_hub_on_the_watch_loop() {
    let mut h = Harness::new().await;
    install(h.push.clone());
    assert_eq!(
        set_token(phone("phone-a"), token(TOKEN_A, &[PushKind::Finished])),
        ActionOutcome::Done
    );
    presence(phone("phone-a"), None);
    h.hub.apply([agent(Activity::Busy)]);
    tokio::spawn(h.push.clone().watch());
    // Let the loop take the first state before the turn ends.
    tokio::time::sleep(Duration::from_millis(50)).await;
    h.hub.apply([agent(Activity::DoneUnread)]);
    while h.clock.sleepers() == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    h.clock.advance(HOLD);
    let got = tokio::time::timeout(Duration::from_secs(10), h.fake.got.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.path, format!("/3/device/{TOKEN_A}"));
    assert_eq!(opened(&got, phone("phone-a"))["title"], TITLE_TEXT);
    forget(phone("phone-a"));
}
