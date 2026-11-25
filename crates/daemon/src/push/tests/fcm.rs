// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Pushes to Android phones, against a local FCM that is also Google's
//! token endpoint.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Response, StatusCode};
use hyper_util::rt::TokioIo;
use maxhq_protocol::attention::AttentionKind;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::Activity;
use maxhq_protocol::phones::{PushEnvironment, PushKind, PushService, PushToken};
use maxhq_protocol::Secret;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use super::{
    agent, agent_id, allow_deny, attention_id, phone, phone_private, prompt, token, Harness, ALL,
    HOLD, SUMMARY, TITLE_TEXT, TOKEN_B,
};
use crate::push::fcm::Data;
use crate::push::seal::open;
use crate::push::{Outgoing, Sent, Via};

/// The test service account's project and email.
pub(super) const PROJECT: &str = "maxhq-test";
pub(super) const EMAIL: &str = "pusher@maxhq-test.iam.example.invalid";

/// Registration tokens in FCM's shape, made up.
const FCM_A: &str = "test-fcm-token-a:APA91bHPRgkF3JUikC4ENAHEeMrd41Zxv3hVZjC9KtT8OvPVGJ-hQMRKRrZuJAEcl7B338qju59zJMjw2DELjzEvxwYv7hH5Ynpc1ODQ0aT4U4OFEeco8ohsN5PjL1iC2dNtk2BAokeMCg2ZXKqpc8FXKmhX94kIxQ";
const FCM_B: &str = "test-fcm-token-b:APA91bE_x-0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-0123456789";
/// The token request's form, up to the assertion.
const GRANT: &str = "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion=";
const SEND: &str = "/v1/projects/maxhq-test/messages:send";

/// What the fake got.
#[derive(Debug)]
pub(super) struct Call {
    path: String,
    authorization: Option<String>,
    body: String,
}

impl Call {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap()
    }
}

/// A local FCM and token endpoint. Each FCM token answers 200 unless told
/// otherwise, and the token endpoint gives `test-access-1`, then
/// `test-access-2` and so on, unless told to refuse.
pub(super) struct FcmFake {
    pub(super) url: String,
    got: mpsc::UnboundedReceiver<Call>,
    answers: Arc<Mutex<HashMap<String, (u16, String)>>>,
    refusal: Arc<Mutex<Option<(u16, String)>>>,
}

impl FcmFake {
    pub(super) async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (sent, got) = mpsc::unbounded_channel();
        let answers: Arc<Mutex<HashMap<String, (u16, String)>>> = Arc::default();
        let refusal: Arc<Mutex<Option<(u16, String)>>> = Arc::default();
        let grants = Arc::new(AtomicUsize::new(0));
        let (held, refused) = (answers.clone(), refusal.clone());
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let (sent, answers, refusal, grants) =
                    (sent.clone(), held.clone(), refused.clone(), grants.clone());
                let service = service_fn(move |request: hyper::Request<Incoming>| {
                    let (sent, answers, refusal, grants) = (
                        sent.clone(),
                        answers.clone(),
                        refusal.clone(),
                        grants.clone(),
                    );
                    async move {
                        let path = request.uri().path().to_owned();
                        let authorization = request
                            .headers()
                            .get("authorization")
                            .map(|v| v.to_str().unwrap().to_owned());
                        let bytes = request.into_body().collect().await.unwrap().to_bytes();
                        let body = String::from_utf8(bytes.to_vec()).unwrap();
                        let (status, answer) = if path == "/token" {
                            let refusal = refusal.lock().unwrap().clone();
                            refusal.unwrap_or_else(|| {
                                let n = grants.fetch_add(1, Ordering::SeqCst) + 1;
                                let granted = json!({
                                    "access_token": format!("test-access-{n}"),
                                    "expires_in": 3_599,
                                    "token_type": "Bearer",
                                });
                                (200, granted.to_string())
                            })
                        } else {
                            let to = serde_json::from_str::<Value>(&body)
                                .ok()
                                .and_then(|v| v["message"]["token"].as_str().map(str::to_owned))
                                .unwrap_or_default();
                            let answer = answers.lock().unwrap().get(&to).cloned();
                            answer.unwrap_or_else(|| {
                                let name = json!({ "name": "projects/maxhq-test/messages/1" });
                                (200, name.to_string())
                            })
                        };
                        let _ = sent.send(Call {
                            path,
                            authorization,
                            body,
                        });
                        let mut response = Response::new(Full::new(Bytes::from(answer)));
                        *response.status_mut() = StatusCode::from_u16(status).unwrap();
                        Ok::<_, Infallible>(response)
                    }
                });
                tokio::spawn(http1::Builder::new().serve_connection(TokioIo::new(stream), service));
            }
        });
        Self {
            url,
            got,
            answers,
            refusal,
        }
    }

    /// Makes FCM answer `status` with `error` for `token`.
    fn answer(&self, token: &str, status: u16, error: &Value) {
        self.answers
            .lock()
            .unwrap()
            .insert(token.to_owned(), (status, error.to_string()));
    }

    /// Makes the token endpoint refuse with `status` and `error`.
    fn refuse(&self, status: u16, error: &Value) {
        *self.refusal.lock().unwrap() = Some((status, error.to_string()));
    }

    fn take(&mut self) -> Vec<Call> {
        let mut got = Vec::new();
        while let Ok(one) = self.got.try_recv() {
            got.push(one);
        }
        got
    }
}

fn fcm_token(token: &str, kinds: &[PushKind]) -> PushToken {
    PushToken {
        token: Secret::new(token.into()),
        topic: "com.example.maxhq".into(),
        environment: PushEnvironment::Production,
        kinds: kinds.to_vec(),
        service: PushService::Fcm,
    }
}

/// FCM's error body, as its docs give it.
fn fcm_error(code: u16, status: &str, error_code: Option<&str>) -> Value {
    let details: Vec<Value> = error_code
        .into_iter()
        .map(|error_code| {
            json!({
                "@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError",
                "errorCode": error_code,
            })
        })
        .collect();
    json!({ "error": { "code": code, "message": "test", "status": status, "details": details } })
}

/// A push about a finished turn to `phone-a`'s FCM token `to`.
fn finished(h: &Harness, to: &str) -> Outgoing {
    Outgoing {
        phone: phone("phone-a"),
        token: fcm_token(to, ALL),
        via: Via::Fcm {
            key: h.fcm_key.clone(),
            data: Data {
                c: STANDARD.encode(b"test-not-sealed"),
                e: None,
                t: agent_id().to_string(),
                k: PushKind::Finished,
                a: None,
                n: "done:test".into(),
            },
        },
    }
}

fn paths(calls: &[Call]) -> Vec<&str> {
    calls.iter().map(|call| call.path.as_str()).collect()
}

#[tokio::test]
async fn an_android_phone_gets_a_data_only_push_sealed_like_an_iphones() {
    let mut h = Harness::new().await;
    let set = |name: &str, push: PushToken| h.push.set_token(phone(name), push);
    assert_eq!(
        set("phone-a", fcm_token(FCM_A, &[PushKind::NeedsInput])),
        ActionOutcome::Done
    );
    assert_eq!(
        set("phone-b", token(TOKEN_B, &[PushKind::NeedsInput])),
        ActionOutcome::Done
    );
    let mut watch = h.watch();
    watch.observe(vec![agent(Activity::Busy)]);
    watch.observe(prompt(AttentionKind::Permission, SUMMARY, allow_deny()));
    h.clock.advance(HOLD);
    assert_eq!(h.send_all(&mut watch).await, [Sent::Done, Sent::Done]);

    // The iPhone's push went to APNs as before.
    let apns = h.fake.take();
    let [iphone] = &apns[..] else {
        panic!("{apns:?}");
    };
    assert_eq!(iphone.path, format!("/3/device/{TOKEN_B}"));

    let calls = h.fcm.take();
    let [grant, message] = &calls[..] else {
        panic!("{calls:?}");
    };
    assert_eq!(grant.path, "/token");
    assert!(grant.body.starts_with(GRANT), "{}", grant.body);
    assert_eq!(message.path, SEND);
    assert_eq!(
        message.authorization.as_deref(),
        Some("Bearer test-access-1")
    );
    let body = message.json();
    let sent = &body["message"];
    assert_eq!(sent["token"], FCM_A);
    assert!(sent.get("notification").is_none(), "data only");
    assert_eq!(
        sent["android"],
        json!({
            "priority": "high",
            "collapse_key": iphone.headers["apns-collapse-id"],
            "ttl": "3600s",
        })
    );
    let data = sent["data"].as_object().unwrap();
    assert!(
        data.values().all(Value::is_string),
        "FCM takes strings only"
    );
    assert_eq!(data["t"], json!(agent_id().to_string()));
    assert_eq!(data["e"], json!(attention_id().to_string()));
    assert_eq!(data["k"], "needs_input");
    assert_eq!(data["a"], "permission");
    assert_eq!(data["n"], json!(iphone.headers["apns-collapse-id"]));
    assert!(!message.body.contains(SUMMARY), "nothing in the clear");
    assert!(!message.body.contains(TITLE_TEXT), "nothing in the clear");

    // The sealed part is what the iPhone gets, sealed to this phone.
    let sealed = STANDARD.decode(data["c"].as_str().unwrap()).unwrap();
    let opened: Value =
        serde_json::from_slice(&open(&phone_private(phone("phone-a")), &sealed).unwrap()).unwrap();
    assert_eq!(opened["title"], TITLE_TEXT);
    assert_eq!(opened["attention"], json!(attention_id()));
    assert_eq!(opened["options"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn the_access_token_is_reused_until_shortly_before_it_expires() {
    let mut h = Harness::new().await;
    assert_eq!(h.push.send(finished(&h, FCM_A)).await, Sent::Done);
    assert_eq!(h.push.send(finished(&h, FCM_B)).await, Sent::Done);
    h.clock.advance(Duration::from_secs(54 * 60));
    assert_eq!(h.push.send(finished(&h, FCM_A)).await, Sent::Done);
    let calls = h.fcm.take();
    assert_eq!(paths(&calls), ["/token", SEND, SEND, SEND], "one token");
    assert!(calls[1..]
        .iter()
        .all(|call| call.authorization.as_deref() == Some("Bearer test-access-1")));

    // Google gave it for 3,599 seconds, so it's done with at 55 minutes.
    h.clock.advance(Duration::from_secs(60));
    assert_eq!(h.push.send(finished(&h, FCM_A)).await, Sent::Done);
    let calls = h.fcm.take();
    assert_eq!(paths(&calls), ["/token", SEND]);
    assert_eq!(
        calls[1].authorization.as_deref(),
        Some("Bearer test-access-2")
    );
}

#[tokio::test]
async fn a_refused_access_token_gets_one_new_one() {
    let mut h = Harness::new().await;
    h.fcm
        .answer(FCM_A, 401, &fcm_error(401, "UNAUTHENTICATED", None));
    assert_eq!(h.push.send(finished(&h, FCM_A)).await, Sent::Expired);
    let calls = h.fcm.take();
    assert_eq!(
        paths(&calls),
        ["/token", SEND, "/token", SEND],
        "tried again once"
    );
    assert_eq!(
        calls[1].authorization.as_deref(),
        Some("Bearer test-access-1")
    );
    assert_eq!(
        calls[3].authorization.as_deref(),
        Some("Bearer test-access-2")
    );
}

#[tokio::test]
async fn a_token_fcm_doesnt_know_is_dropped() {
    let mut h = Harness::new().await;
    for (name, to) in [("phone-a", FCM_A), ("phone-b", FCM_B)] {
        assert_eq!(
            h.push.set_token(phone(name), fcm_token(to, ALL)),
            ActionOutcome::Done
        );
    }
    h.fcm.answer(
        FCM_A,
        404,
        &fcm_error(404, "NOT_FOUND", Some("UNREGISTERED")),
    );
    assert_eq!(h.push.send(finished(&h, FCM_A)).await, Sent::Gone);
    assert_eq!(h.fcm.take().len(), 2, "not tried again");
    let state = h.push.lock();
    assert!(state.tokens.get(phone("phone-a")).is_none());
    assert!(state.tokens.get(phone("phone-b")).is_some());
}

#[tokio::test]
async fn a_key_google_refuses_fails_without_sending() {
    let mut h = Harness::new().await;
    h.fcm.refuse(
        400,
        &json!({ "error": "invalid_grant", "error_description": "Invalid JWT Signature." }),
    );
    let sent = h.push.send(finished(&h, FCM_A)).await;
    assert_eq!(
        sent,
        Sent::Failed(
            "Google refused the Firebase key: invalid_grant: Invalid JWT Signature.".into()
        )
    );
    assert_eq!(paths(&h.fcm.take()), ["/token"]);
}

#[tokio::test]
async fn each_phone_needs_its_own_services_key() {
    let mut h = Harness::new().await;
    let push = crate::push::Push::open(
        h.hub.clone(),
        crate::secrets::files::Folder::open(h._temp.path().join("other")).unwrap(),
        h.clock.clone(),
        crate::push::Services {
            apns: crate::push::Apns::at(h.fake.url.clone()),
            apns_key: Arc::new(|| None),
            fcm: crate::push::Fcm::at(&h.fcm.url),
            fcm_key: Arc::new({
                let key = h.fcm_key.clone();
                move || Some(key.clone())
            }),
        },
    );
    push.set_token(phone("phone-a"), fcm_token(FCM_A, &[PushKind::Finished]));
    push.set_token(phone("phone-b"), token(TOKEN_B, &[PushKind::Finished]));
    let mut watch = crate::push::Watch::new(push.clone());
    watch.reset(h.hub.snapshot().entities, true);
    watch.observe(vec![agent(Activity::Busy)]);
    watch.observe(vec![agent(Activity::DoneUnread)]);
    h.clock.advance(HOLD);
    let due = watch.due();
    assert_eq!(due.len(), 1, "no APNs key, so only the Android phone");
    assert_eq!(due[0].phone, phone("phone-a"));
    assert_eq!(push.send(due.into_iter().next().unwrap()).await, Sent::Done);
    assert_eq!(paths(&h.fcm.take()), ["/token", SEND]);
    assert!(h.fake.take().is_empty());
}

#[tokio::test]
async fn tokens_are_checked_by_their_service() {
    let h = Harness::new().await;
    let set = |push: PushToken| h.push.set_token(phone("phone-a"), push);
    let with = |to: &str, topic: &str, service: PushService| PushToken {
        topic: topic.into(),
        service,
        ..fcm_token(to, ALL)
    };
    assert_eq!(set(fcm_token(FCM_A, ALL)), ActionOutcome::Done);
    assert_eq!(
        set(with(FCM_A, "com.example.max_hq", PushService::Fcm)),
        ActionOutcome::Done,
        "a package name may have underscores"
    );
    for (push, reason) in [
        // An APNs token is hex.
        (
            with(FCM_A, "com.example.maxhq", PushService::Apns),
            "That isn't an APNs device token.",
        ),
        (
            with("abc", "com.example.maxhq", PushService::Fcm),
            "That isn't an FCM registration token.",
        ),
        (
            with(
                &format!("{FCM_A}/../x"),
                "com.example.maxhq",
                PushService::Fcm,
            ),
            "That isn't an FCM registration token.",
        ),
        (
            with(&format!("{FCM_A} x"), "com.example.maxhq", PushService::Fcm),
            "That isn't an FCM registration token.",
        ),
        (
            with(&"a".repeat(1_025), "com.example.maxhq", PushService::Fcm),
            "That isn't an FCM registration token.",
        ),
        (
            with(FCM_A, "", PushService::Fcm),
            "That isn't an Android package name.",
        ),
        (
            with(FCM_A, "com.example-maxhq", PushService::Fcm),
            "That isn't an Android package name.",
        ),
    ] {
        assert_eq!(
            set(push),
            ActionOutcome::Refused {
                reason: reason.into()
            }
        );
    }
    // A hex token from an iPhone still goes as one.
    assert_eq!(
        set(with(TOKEN_B, "com.example.maxhq", PushService::Apns)),
        ActionOutcome::Done
    );
}
