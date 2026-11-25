// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::collections::HashMap;
use std::fs;
use std::time::Duration;

use maxhq_protocol::guest::TokenReply;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{Credential, CredentialKind, Vendor};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::Secret;
use serde_json::Value;
use url::Url;

use super::fake::{Callback, Fake, Spoil, EMAIL};
use super::{chatgpt_account, credential_id, ENDED, NO_PLAN};
use crate::secrets::testing::{hub_for, Clock};
use crate::secrets::{github, Credentials, Secrets, CHATGPT_FILE, DIR};
use crate::testing::TempDir;

/// A device's store against the fake server.
struct Device {
    temp: TempDir,
    name: &'static str,
    secrets: Secrets,
}

impl Device {
    fn open(name: &'static str, fake: &Fake, clock: &Clock) -> Self {
        let temp = TempDir::new();
        let secrets = open(&temp, name, fake, clock);
        Self {
            temp,
            name,
            secrets,
        }
    }

    fn reopen(&self, fake: &Fake, clock: &Clock) -> Secrets {
        open(&self.temp, self.name, fake, clock)
    }

    fn file(&self) -> Value {
        let text = fs::read_to_string(self.temp.path().join(DIR).join(CHATGPT_FILE)).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn credential(&self) -> Option<Credential> {
        let id = credential_id(self.secrets.device());
        match self.secrets.inner.hub.get(&EntityKey::Credential(id)) {
            Some(Entity::Credential(credential)) => Some(credential),
            _ => None,
        }
    }

    async fn token(&self) -> TokenReply {
        let account = chatgpt_account(self.secrets.device());
        self.secrets.chatgpt_token(account).await
    }

    /// The page a new round opens, and the callback it gets.
    async fn begin(
        &self,
        fake: &Fake,
    ) -> (
        maxhq_protocol::ids::SignInId,
        HashMap<String, String>,
        Callback,
    ) {
        let ActionOutcome::SignIn {
            sign_in,
            url,
            callback_port,
        } = self.secrets.begin_chatgpt().await
        else {
            panic!("no round started");
        };
        let query: HashMap<String, String> = Url::parse(&url)
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        assert_eq!(
            query["redirect_uri"],
            format!("http://127.0.0.1:{callback_port}/auth/callback")
        );
        (sign_in, query, fake.authorize(&url))
    }

    async fn sign_in(&self, fake: &Fake) -> ActionOutcome {
        let (sign_in, _, callback) = self.begin(fake).await;
        self.finish(sign_in, callback).await
    }

    async fn finish(
        &self,
        sign_in: maxhq_protocol::ids::SignInId,
        callback: Callback,
    ) -> ActionOutcome {
        self.secrets
            .finish_chatgpt(
                sign_in,
                Secret::new(callback.code),
                callback.state,
                callback.client_id,
            )
            .await
    }
}

fn open(temp: &TempDir, name: &str, fake: &Fake, clock: &Clock) -> Secrets {
    Secrets::open_with(
        temp.path().join(DIR),
        hub_for(name),
        github::Api::local("http://127.0.0.1:9"),
        github::Gh::fake(|_| unreachable!("no test here runs gh")),
        fake.issuer(),
        clock.now(),
    )
    .unwrap()
}

async fn rig() -> (Fake, Clock) {
    let clock = Clock::at("2026-10-31T12:00:00Z");
    (Fake::start(clock.now()).await, clock)
}

fn token(reply: TokenReply) -> String {
    match reply {
        TokenReply::Token { value, .. } => value.expose().to_owned(),
        TokenReply::Refused { reason } => panic!("refused: {reason}"),
    }
}

fn refused(outcome: ActionOutcome) -> String {
    match outcome {
        ActionOutcome::Refused { reason } => reason,
        other => panic!("not refused: {other:?}"),
    }
}

#[tokio::test]
async fn a_first_sign_in_registers_and_stores_the_session() {
    let (fake, clock) = rig().await;
    let device = Device::open("devbox", &fake, &clock);
    let (sign_in, query, callback) = device.begin(&fake).await;
    assert_eq!(query["client_id"], "dynamic_agent_client");
    assert_eq!(query["agent_name_hint"], "MaxHQ");
    assert!(query["scope"]
        .split(' ')
        .any(|s| s == "chatgpt.tokens.use.direct"));
    assert!(!query.contains_key("login_hint"));
    assert!(!query.contains_key("id_token_hint"));
    let host_id = query["ext_agent_host_id"].clone();
    assert_eq!(device.file()["host_id"], host_id.as_str());

    assert_eq!(device.finish(sign_in, callback).await, ActionOutcome::Done);
    let file = device.temp.path().join(DIR).join(CHATGPT_FILE);
    assert_eq!(
        maxhq_os::fsmode::mode_of(&fs::metadata(&file).unwrap()) & 0o777,
        0o600
    );
    assert_eq!(token(device.token().await), "test-access-2");
    let account = chatgpt_account(device.secrets.device());
    assert!(device.secrets.has_chatgpt(account));
    let credential = device.credential().unwrap();
    assert_eq!(
        credential.kind,
        CredentialKind::ChatGpt {
            account,
            email: Some(EMAIL.into()),
        }
    );
    assert_eq!(credential.problem, None);
    match device.secrets.inner.hub.get(&EntityKey::Account(account)) {
        Some(Entity::Account(account)) => {
            assert_eq!(account.vendor, Vendor::Codex);
            assert_eq!(account.label, EMAIL);
        }
        other => panic!("no account: {other:?}"),
    }

    // Read back, with the same host ID, and the next round signs in
    // through the registered client.
    let again = device.reopen(&fake, &clock);
    assert!(again.has_chatgpt(account));
    let device = Device {
        secrets: again,
        ..device
    };
    assert!(device.credential().is_some());
    let (_, query, _) = device.begin(&fake).await;
    assert_eq!(query["ext_agent_host_id"], host_id);
    assert_eq!(query["client_id"], fake.state().clients[&host_id]);
    assert_eq!(query["login_hint"], EMAIL);
    assert!(!query.contains_key("agent_name_hint"));
}

#[tokio::test]
async fn a_callback_that_doesnt_match_its_round_is_refused() {
    let (fake, clock) = rig().await;
    let device = Device::open("devbox", &fake, &clock);

    let (sign_in, _, mut callback) = device.begin(&fake).await;
    callback.state = "test-other-state".into();
    assert!(refused(device.finish(sign_in, callback).await).contains("couldn't be verified"));

    // A round is used once.
    let (sign_in, _, callback) = device.begin(&fake).await;
    let code = callback.code.clone();
    let state = callback.state.clone();
    assert_eq!(device.finish(sign_in, callback).await, ActionOutcome::Done);
    let replay = Callback {
        code,
        state,
        client_id: None,
    };
    assert!(refused(device.finish(sign_in, replay).await).contains("ended"));

    // A first sign-in needs the client it registered.
    let other = Device::open("devbox-2", &fake, &clock);
    let (sign_in, _, mut callback) = other.begin(&fake).await;
    callback.client_id = Some("not an issued client".into());
    assert!(refused(other.finish(sign_in, callback).await).contains("didn't finish registering"));
    let (sign_in, _, mut callback) = other.begin(&fake).await;
    callback.client_id = None;
    assert!(refused(other.finish(sign_in, callback).await).contains("didn't finish registering"));
    assert!(other.credential().is_none());
}

#[tokio::test]
async fn an_identity_token_that_fails_a_check_is_refused() {
    let (fake, clock) = rig().await;
    let device = Device::open("devbox", &fake, &clock);
    for spoil in [
        Spoil::Nonce,
        Spoil::Audience,
        Spoil::Signature,
        Spoil::Expired,
    ] {
        fake.state().spoil = spoil;
        let reason = refused(device.sign_in(&fake).await);
        assert!(reason.contains("couldn't be verified"), "{reason}");
        assert!(device.credential().is_none());
    }
    fake.state().spoil = Spoil::Nothing;
    assert_eq!(device.sign_in(&fake).await, ActionOutcome::Done);
}

#[tokio::test]
async fn a_token_is_renewed_shortly_before_it_runs_out() {
    let (fake, clock) = rig().await;
    let device = Device::open("devbox", &fake, &clock);
    device.sign_in(&fake).await;
    let first = token(device.token().await);
    assert_eq!(token(device.token().await), first);
    assert_eq!(fake.state().refreshes, 0);

    clock.advance(Duration::from_secs(3600 - 30));
    let renewed = token(device.token().await);
    assert_ne!(renewed, first);
    assert_eq!(fake.state().refreshes, 1);
    assert_eq!(device.file()["session"]["refresh_token"], "test-refresh-3");

    // The rotated session is what a restart reads.
    let again = device.reopen(&fake, &clock);
    let account = chatgpt_account(again.device());
    assert_eq!(token(again.chatgpt_token(account).await), renewed);
    assert_eq!(fake.state().refreshes, 1);
}

#[tokio::test]
async fn renewals_go_one_at_a_time() {
    let (fake, clock) = rig().await;
    let device = Device::open("devbox", &fake, &clock);
    device.sign_in(&fake).await;
    fake.state().refresh_delay = Duration::from_millis(200);
    clock.advance(Duration::from_secs(3600));

    let (a, b, c) = tokio::join!(device.token(), device.token(), device.token());
    let renewed = token(a);
    assert_eq!(token(b), renewed);
    assert_eq!(token(c), renewed);
    // A second renewal with the spent token would have ended the session.
    assert_eq!(fake.state().refreshes, 1);
}

#[tokio::test]
async fn a_session_that_cant_be_renewed_is_cleared() {
    let (fake, clock) = rig().await;
    let device = Device::open("devbox", &fake, &clock);
    device.sign_in(&fake).await;
    let host_id = device.file()["host_id"].clone();
    fake.state().live.clear();
    clock.advance(Duration::from_secs(3600));

    match device.token().await {
        TokenReply::Refused { reason } => assert_eq!(reason, ENDED),
        other => panic!("not refused: {other:?}"),
    }
    assert!(device.credential().is_none());
    assert!(!device
        .secrets
        .has_chatgpt(chatgpt_account(device.secrets.device())));
    let file = device.file();
    assert_eq!(file["session"], Value::Null);
    assert_eq!(file["host_id"], host_id);
    assert!(file["registration"]["client_id"].is_string());
}

#[tokio::test]
async fn signing_out_revokes_then_clears() {
    let (fake, clock) = rig().await;
    let device = Device::open("devbox", &fake, &clock);
    device.sign_in(&fake).await;
    let credential = device.credential().unwrap().id;

    assert_eq!(
        device.secrets.forget(credential).await,
        ActionOutcome::SignedOut { revoked: true }
    );
    assert_eq!(fake.state().revoked, ["test-refresh-2"]);
    assert!(device.credential().is_none());
    assert_eq!(device.file()["session"], Value::Null);
    assert!(matches!(device.token().await, TokenReply::Refused { .. }));
    assert!(matches!(
        device.secrets.forget(credential).await,
        ActionOutcome::Refused { .. }
    ));

    // A server that won't confirm is tried three times, and the session
    // goes all the same.
    device.sign_in(&fake).await;
    fake.state().revoke_status = 503;
    fake.state().revocations = 0;
    assert_eq!(
        device.secrets.forget(credential).await,
        ActionOutcome::SignedOut { revoked: false }
    );
    assert_eq!(fake.state().revocations, 3);
    assert!(device.credential().is_none());
    assert_eq!(device.file()["session"], Value::Null);
}

#[tokio::test]
async fn a_sign_in_without_plan_usage_is_kept_but_gives_no_token() {
    let (fake, clock) = rig().await;
    let device = Device::open("devbox", &fake, &clock);
    fake.state().plan = false;
    assert_eq!(refused(device.sign_in(&fake).await), NO_PLAN);
    assert_eq!(
        device.credential().unwrap().problem.as_deref(),
        Some(NO_PLAN)
    );
    assert!(!device
        .secrets
        .has_chatgpt(chatgpt_account(device.secrets.device())));
    assert!(matches!(device.token().await, TokenReply::Refused { .. }));

    // The next round asks again.
    fake.state().plan = true;
    let (sign_in, query, callback) = device.begin(&fake).await;
    assert_eq!(query["prompt"], "consent");
    assert_eq!(device.finish(sign_in, callback).await, ActionOutcome::Done);
    assert_eq!(device.credential().unwrap().problem, None);
}

#[tokio::test]
async fn a_second_device_signs_in_on_its_own() {
    let (fake, clock) = rig().await;
    let first = Device::open("devbox", &fake, &clock);
    let second = Device::open("devbox-2", &fake, &clock);
    first.sign_in(&fake).await;
    second.sign_in(&fake).await;

    let (a, b) = (first.file(), second.file());
    assert_ne!(a["host_id"], b["host_id"]);
    assert_ne!(
        a["registration"]["client_id"],
        b["registration"]["client_id"]
    );
    assert_ne!(a["session"]["refresh_token"], b["session"]["refresh_token"]);
    assert_ne!(token(first.token().await), token(second.token().await));
    let first_account = chatgpt_account(first.secrets.device());
    assert!(matches!(
        second.secrets.chatgpt_token(first_account).await,
        TokenReply::Refused { .. }
    ));
    assert!(!second.secrets.has_chatgpt(first_account));
}

#[tokio::test]
async fn another_account_needs_a_sign_out_first() {
    let (fake, clock) = rig().await;
    let device = Device::open("devbox", &fake, &clock);
    device.sign_in(&fake).await;
    fake.state().subject = "user-test-2".into();
    assert!(refused(device.sign_in(&fake).await).contains("Sign out"));
    assert_eq!(device.file()["session"]["subject"], "user-test-1");

    // Signed out, the registration goes and the next round registers anew.
    device.secrets.forget(device.credential().unwrap().id).await;
    assert!(refused(device.sign_in(&fake).await).contains("Sign in again"));
    let (_, query, _) = device.begin(&fake).await;
    assert_eq!(query["client_id"], "dynamic_agent_client");
}

#[tokio::test]
async fn a_stale_code_keeps_the_registration() {
    let (fake, clock) = rig().await;
    let device = Device::open("devbox", &fake, &clock);
    fake.state().stale_codes = true;
    assert!(refused(device.sign_in(&fake).await).contains("ran out"));
    let (_, query, _) = device.begin(&fake).await;
    assert_ne!(query["client_id"], "dynamic_agent_client");
    assert_eq!(
        query["client_id"],
        device.file()["registration"]["client_id"]
    );
}
