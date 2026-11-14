// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Your own sign-in: the device flow, the session's refreshes and sign-out,
//! against the fake GitHub.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use maxhq_protocol::ids::SignInId;

use super::super::api::LoginReply;
use super::super::fake::{
    CLIENT_ID, DEVICE_CODE, LOGIN, REFRESH_PREFIX, USER_CODE, USER_TOKEN_PREFIX,
};
use super::super::user::{polled, Polled, DENIED, EXPIRED, GONE};
use super::*;
use crate::secrets::GITHUB_USER_FILE;
use crate::session::clock::FakeClock;

const HOUR: Duration = Duration::from_secs(60 * 60);
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
/// How long the fake's access tokens last.
const LASTS: Duration = Duration::from_secs(8 * 60 * 60);

impl Rig {
    /// Makes polling wait on a clock the test moves.
    fn polling(&self) -> Arc<FakeClock> {
        let clock = Arc::new(FakeClock::new());
        self.secrets.inner.github.user.set_clock(clock.clone());
        clock
    }

    async fn begin(&self, client_id: Option<&str>) -> ActionOutcome {
        self.secrets
            .begin_github_user(client_id.map(str::to_owned))
            .await
    }

    /// Signs in through the device flow with the client ID the app passes.
    async fn sign_in(&self) -> Arc<FakeClock> {
        let clock = self.polling();
        assert!(matches!(
            self.begin(Some(CLIENT_ID)).await,
            ActionOutcome::DeviceCode { .. }
        ));
        poll(&clock, 5).await;
        eventually(|| signed_in(&self.secrets)).await;
        clock
    }

    fn user_file(&self) -> Option<Value> {
        let text = fs::read_to_string(self.folder().join(GITHUB_USER_FILE)).ok()?;
        Some(serde_json::from_str(&text).unwrap())
    }

    /// The access token the file holds.
    fn stored_token(&self) -> String {
        self.user_file().unwrap()["access_token"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// The bodies of the access token requests with `grant`.
    fn trades(&self, grant: &str) -> Vec<Value> {
        self.fake
            .state()
            .requests
            .iter()
            .filter(|request| request.path == "/login/oauth/access_token")
            .filter(|request| request.body["grant_type"] == grant)
            .map(|request| request.body.clone())
            .collect()
    }

    fn polls(&self) -> usize {
        self.trades("urn:ietf:params:oauth:grant-type:device_code")
            .len()
    }

    fn refreshes(&self) -> Vec<Value> {
        self.trades("refresh_token")
    }
}

/// Waits for the device flow to go to sleep, then moves its clock `secs`.
async fn poll(clock: &FakeClock, secs: u64) {
    eventually(|| (clock.sleepers() == 1).then_some(())).await;
    clock.advance(Duration::from_secs(secs));
}

/// Lets the device flow do whatever it would.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(50)).await;
}

/// The login the hub shows signed in on this device, if any.
fn signed_in(secrets: &Secrets) -> Option<String> {
    secrets
        .inner
        .hub
        .snapshot()
        .entities
        .into_iter()
        .find_map(|entity| match entity {
            Entity::Credential(Credential {
                kind: CredentialKind::GithubUser { login },
                device,
                ..
            }) if device == secrets.device() => Some(login),
            _ => None,
        })
}

fn user_credential(secrets: &Secrets) -> CredentialId {
    CredentialId::derive(&format!("github-user:{}", secrets.device()))
}

fn refusal(outcome: ActionOutcome) -> String {
    let ActionOutcome::Refused { reason } = outcome else {
        panic!("not refused: {outcome:?}");
    };
    reason
}

fn reply(error: &str, interval: Option<u64>) -> LoginReply {
    serde_json::from_value(json!({ "error": error, "interval": interval })).unwrap()
}

#[test]
fn each_poll_answer_means_what_github_says() {
    let token: LoginReply = serde_json::from_value(json!({ "access_token": "t" })).unwrap();
    assert_eq!(polled(&token, 5), Polled::Token);
    assert_eq!(
        polled(&reply("authorization_pending", None), 5),
        Polled::Wait(5)
    );
    assert_eq!(polled(&reply("slow_down", None), 5), Polled::Wait(10));
    assert_eq!(polled(&reply("slow_down", Some(15)), 5), Polled::Wait(15));
    assert_eq!(
        polled(&reply("expired_token", None), 5),
        Polled::Failed(EXPIRED.into())
    );
    assert_eq!(
        polled(&reply("access_denied", None), 5),
        Polled::Failed(DENIED.into())
    );
    assert_eq!(
        polled(&reply("unsupported_grant_type", None), 5),
        Polled::Failed("GitHub refused the sign-in: unsupported_grant_type".into())
    );
}

#[tokio::test]
async fn signing_in_stores_the_session_and_shows_who_you_are() {
    let rig = Rig::new().await;
    let clock = rig.polling();
    rig.fake.state().login.polls = ["authorization_pending"].into();
    let ActionOutcome::DeviceCode {
        sign_in,
        user_code,
        verification_uri,
        expires_at,
    } = rig.begin(Some(CLIENT_ID)).await
    else {
        panic!("no device code");
    };
    assert_eq!(user_code, USER_CODE);
    assert_eq!(verification_uri, "https://github.com/login/device");
    assert_eq!(expires_at, at("2026-10-25T12:15:00Z"));

    poll(&clock, 5).await;
    eventually(|| (rig.polls() == 1).then_some(())).await;
    assert_eq!(signed_in(&rig.secrets), None);
    poll(&clock, 5).await;
    assert_eq!(eventually(|| signed_in(&rig.secrets)).await, LOGIN);
    assert_eq!(rig.secrets.check_github_user(sign_in), ActionOutcome::Done);

    let path = rig.folder().join(GITHUB_USER_FILE);
    assert_eq!(mode(&path), 0o600);
    let file = rig.user_file().unwrap();
    assert_eq!(file["access_token"], format!("{USER_TOKEN_PREFIX}1"));
    assert_eq!(file["refresh_token"], format!("{REFRESH_PREFIX}1"));
    assert_eq!(file["login"], LOGIN);
    assert_eq!(file["user_id"], 583_231);
    assert_eq!(file["client_id"], CLIENT_ID);
    assert!(file.get("refused").is_none());
    let polls = rig.trades("urn:ietf:params:oauth:grant-type:device_code");
    assert_eq!(polls[0]["device_code"], DEVICE_CODE);
    assert_eq!(polls[0]["client_id"], CLIENT_ID);

    let token = rig.secrets.github_user().await.unwrap();
    assert_eq!(token.login, LOGIN);
    assert_eq!(token.value.expose(), format!("{USER_TOKEN_PREFIX}1"));
    // The sign-in outlasts a restart.
    let again = rig.reopen();
    assert_eq!(signed_in(&again).as_deref(), Some(LOGIN));
    assert_eq!(again.github_user().await.unwrap().login, LOGIN);
}

#[tokio::test]
async fn slow_down_waits_longer_between_polls() {
    let rig = Rig::new().await;
    let clock = rig.polling();
    rig.fake.state().login.polls = ["slow_down"].into();
    rig.begin(Some(CLIENT_ID)).await;
    poll(&clock, 5).await;
    eventually(|| (rig.polls() == 1).then_some(())).await;

    // Now ten seconds apart, not five.
    poll(&clock, 9).await;
    settle().await;
    assert_eq!(rig.polls(), 1);
    clock.advance(Duration::from_secs(1));
    eventually(|| signed_in(&rig.secrets)).await;
    assert_eq!(rig.polls(), 2);
}

#[tokio::test]
async fn a_flow_github_ends_says_why() {
    for (answer, why) in [("expired_token", EXPIRED), ("access_denied", DENIED)] {
        let rig = Rig::new().await;
        rig.polling();
        rig.fake.state().login.polls = [answer].into();
        let polled = rig
            .secrets
            .poll_github_user(CLIENT_ID, &Secret::new(DEVICE_CODE.into()), 900, Some(0))
            .await;
        assert_eq!(polled.err().as_deref(), Some(why));
    }
}

#[tokio::test]
async fn a_code_you_never_enter_runs_out() {
    let rig = Rig::new().await;
    let clock = rig.polling();
    rig.fake.state().login.polls = ["authorization_pending"; 300].into();
    rig.begin(Some(CLIENT_ID)).await;
    poll(&clock, 900).await;
    eventually(|| (clock.sleepers() == 0).then_some(())).await;
    settle().await;
    assert_eq!(rig.polls(), 0);
    assert_eq!(signed_in(&rig.secrets), None);
    assert!(rig.user_file().is_none());

    let polled = rig
        .secrets
        .poll_github_user(CLIENT_ID, &Secret::new(DEVICE_CODE.into()), 0, Some(0))
        .await;
    assert_eq!(polled.err().as_deref(), Some(EXPIRED));
}

#[tokio::test]
async fn a_failed_flow_signs_no_one_in_and_says_why_when_asked() {
    let rig = Rig::new().await;
    let clock = rig.polling();
    rig.fake.state().login.polls = ["access_denied"].into();
    let ActionOutcome::DeviceCode { sign_in, .. } = rig.begin(Some(CLIENT_ID)).await else {
        panic!("no device code");
    };
    assert_eq!(rig.secrets.check_github_user(sign_in), ActionOutcome::Done);
    poll(&clock, 5).await;
    eventually(|| (rig.polls() == 1).then_some(())).await;
    eventually(|| (clock.sleepers() == 0).then_some(())).await;
    settle().await;
    assert_eq!(signed_in(&rig.secrets), None);
    assert!(rig.user_file().is_none());
    assert_eq!(refusal(rig.secrets.check_github_user(sign_in)), DENIED);
    // One this device doesn't know, as after a restart, isn't waiting.
    let other = SignInId::derive("other");
    assert_eq!(refusal(rig.secrets.check_github_user(other)), GONE);
}

#[tokio::test]
async fn a_second_sign_in_replaces_the_first() {
    let rig = Rig::new().await;
    let clock = rig.polling();
    let ActionOutcome::DeviceCode { sign_in: first, .. } = rig.begin(Some(CLIENT_ID)).await else {
        panic!("no device code");
    };
    let ActionOutcome::DeviceCode {
        sign_in: second, ..
    } = rig.begin(Some(CLIENT_ID)).await
    else {
        panic!("no device code");
    };
    assert_ne!(first, second);
    settle().await;
    poll(&clock, 5).await;
    eventually(|| signed_in(&rig.secrets)).await;
    settle().await;
    // Only the second flow polled.
    assert_eq!(rig.polls(), 1);
    // Cancelling the replaced flow touches nothing.
    assert_eq!(rig.secrets.cancel_github_user(first), ActionOutcome::Done);
    assert!(signed_in(&rig.secrets).is_some());
}

#[tokio::test]
async fn cancel_stops_polling() {
    let rig = Rig::new().await;
    let clock = rig.polling();
    let ActionOutcome::DeviceCode { sign_in, .. } = rig.begin(Some(CLIENT_ID)).await else {
        panic!("no device code");
    };
    eventually(|| (clock.sleepers() == 1).then_some(())).await;
    assert_eq!(rig.secrets.cancel_github_user(sign_in), ActionOutcome::Done);
    eventually(|| (clock.sleepers() == 0).then_some(())).await;
    clock.advance(Duration::from_secs(60));
    settle().await;
    assert_eq!(rig.polls(), 0);
    assert_eq!(signed_in(&rig.secrets), None);
    // A second cancel is still done.
    assert_eq!(rig.secrets.cancel_github_user(sign_in), ActionOutcome::Done);
}

#[tokio::test]
async fn device_flow_off_says_where_to_turn_it_on() {
    let rig = Rig::new().await;
    assert_eq!(rig.set(), ActionOutcome::Done);
    assert!(rig.secrets.check_github().await);
    rig.fake.state().login.device_flow = false;
    let reason = refusal(rig.begin(None).await);
    assert!(reason.contains("Enable Device Flow"), "{reason}");
    assert!(
        reason.contains(&format!("https://github.com/settings/apps/{SLUG}")),
        "{reason}"
    );
}

#[tokio::test]
async fn a_sign_in_needs_a_client_id() {
    let rig = Rig::new().await;
    let reason = refusal(rig.begin(None).await);
    assert!(
        reason.contains("no GitHub App to sign in through"),
        "{reason}"
    );
    for bad in ["", "Iv23 test", "Iv23/../x", &"a".repeat(101)] {
        assert!(matches!(
            rig.begin(Some(bad)).await,
            ActionOutcome::Refused { .. }
        ));
    }
    // A client ID GitHub doesn't know.
    let reason = refusal(rig.begin(Some("Iv23unknown")).await);
    assert!(
        reason.contains("GitHub wouldn't start the sign-in"),
        "{reason}"
    );
}

#[tokio::test]
async fn the_apps_own_client_id_is_kept() {
    let rig = Rig::new().await;
    assert_eq!(rig.set(), ActionOutcome::Done);
    assert!(rig.secrets.check_github().await);
    assert_eq!(rig.file()["client_id"], CLIENT_ID);
    // The App's own ID wins over one the app passes.
    assert!(matches!(
        rig.begin(Some("Iv23other")).await,
        ActionOutcome::DeviceCode { .. }
    ));
    let again = rig.reopen();
    assert!(matches!(
        again.begin_github_user(None).await,
        ActionOutcome::DeviceCode { .. }
    ));

    // A file from before client IDs were kept still loads.
    let mut file = rig.file();
    file.as_object_mut().unwrap().remove("client_id");
    fs::write(
        rig.folder().join(GITHUB_APP_FILE),
        serde_json::to_vec(&file).unwrap(),
    )
    .unwrap();
    let old = rig.reopen();
    assert!(credential(&old).is_some());
}

#[tokio::test]
async fn a_fresh_token_is_reused() {
    let rig = Rig::new().await;
    rig.sign_in().await;
    let sent = rig.fake.state().requests.len();
    rig.clock.advance(LASTS - Duration::from_secs(6 * 60));
    let token = rig.secrets.github_user().await.unwrap();
    assert_eq!(token.value.expose(), format!("{USER_TOKEN_PREFIX}1"));
    assert_eq!(rig.fake.state().requests.len(), sent);
}

#[tokio::test]
async fn a_token_near_its_end_is_refreshed_and_stored() {
    let rig = Rig::new().await;
    rig.sign_in().await;
    rig.clock.advance(LASTS - Duration::from_secs(4 * 60));
    let token = rig.secrets.github_user().await.unwrap();
    assert_eq!(token.value.expose(), format!("{USER_TOKEN_PREFIX}2"));
    assert_eq!(rig.stored_token(), format!("{USER_TOKEN_PREFIX}2"));
    assert_eq!(
        rig.user_file().unwrap()["refresh_token"],
        format!("{REFRESH_PREFIX}2")
    );

    let refreshes = rig.refreshes();
    assert_eq!(refreshes.len(), 1);
    assert_eq!(
        refreshes[0],
        json!({
            "client_id": CLIENT_ID,
            "grant_type": "refresh_token",
            "refresh_token": format!("{REFRESH_PREFIX}1"),
        })
    );
    // Then it's fresh again.
    rig.secrets.github_user().await.unwrap();
    assert_eq!(rig.refreshes().len(), 1);
}

#[tokio::test]
async fn asks_at_once_refresh_once() {
    let rig = Rig::new().await;
    rig.sign_in().await;
    rig.clock.advance(LASTS);
    let (one, two) = tokio::join!(rig.secrets.github_user(), rig.secrets.github_user());
    assert_eq!(one.unwrap().value, two.unwrap().value);
    assert_eq!(rig.refreshes().len(), 1);
}

#[tokio::test]
async fn github_ending_the_session_signs_you_out() {
    let rig = Rig::new().await;
    rig.sign_in().await;
    rig.fake.state().login.refresh = None;
    rig.clock.advance(LASTS);
    let err = rig.secrets.github_user().await.unwrap_err();
    assert_eq!(err, GithubError::NotSignedIn);
    assert!(rig.user_file().is_none());
    assert_eq!(signed_in(&rig.secrets), None);
    assert_eq!(rig.refreshes().len(), 1);
}

#[tokio::test]
async fn a_lapsed_refresh_token_signs_you_out_without_asking() {
    let rig = Rig::new().await;
    rig.sign_in().await;
    rig.clock.advance(185 * DAY);
    let err = rig.secrets.github_user().await.unwrap_err();
    assert_eq!(err, GithubError::NotSignedIn);
    assert!(rig.user_file().is_none());
    assert_eq!(signed_in(&rig.secrets), None);
    assert!(rig.refreshes().is_empty());
}

#[tokio::test]
async fn a_failed_refresh_keeps_a_token_that_still_works() {
    let rig = Rig::new().await;
    rig.sign_in().await;
    rig.fake.state().login.down = true;
    rig.clock.advance(LASTS - Duration::from_secs(4 * 60));
    let token = rig.secrets.github_user().await.unwrap();
    assert_eq!(token.value.expose(), format!("{USER_TOKEN_PREFIX}1"));

    rig.clock.advance(Duration::from_secs(5 * 60));
    let err = rig.secrets.github_user().await.unwrap_err();
    assert_ne!(err, GithubError::NotSignedIn);
    // Still signed in, so it works once GitHub is back.
    assert_eq!(signed_in(&rig.secrets).as_deref(), Some(LOGIN));
    assert_eq!(rig.stored_token(), format!("{USER_TOKEN_PREFIX}1"));
    rig.fake.state().login.down = false;
    let token = rig.secrets.github_user().await.unwrap();
    assert_eq!(token.value.expose(), format!("{USER_TOKEN_PREFIX}2"));
}

#[tokio::test]
async fn a_refused_token_is_refreshed_and_tried_once_more() {
    let rig = Rig::new().await;
    rig.sign_in().await;
    rig.fake.state().login.valid.clear();
    let api = &rig.secrets.inner.github.api;
    let you = rig
        .secrets
        .with_github_user(|token| async move { api.user(&token.value).await })
        .await
        .unwrap();
    assert_eq!(you.login, LOGIN);
    assert_eq!(rig.refreshes().len(), 1);
    assert_eq!(rig.stored_token(), format!("{USER_TOKEN_PREFIX}2"));

    // Only once, though.
    let tries = AtomicUsize::new(0);
    let err = rig
        .secrets
        .with_github_user(|_| async {
            tries.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(GithubError::Refused {
                status: 401,
                message: "Bad credentials".into(),
            })
        })
        .await
        .unwrap_err();
    assert!(matches!(err, GithubError::Refused { status: 401, .. }));
    assert_eq!(tries.load(Ordering::SeqCst), 2);
    assert_eq!(rig.refreshes().len(), 2);
}

#[tokio::test]
async fn a_token_that_never_expires_is_never_refreshed() {
    let rig = Rig::new().await;
    rig.fake.state().login.expires_in = None;
    rig.sign_in().await;
    assert!(rig.user_file().unwrap()["access_expires_at"].is_null());
    rig.clock.advance(400 * DAY);
    rig.secrets.keep_github_user().await;
    let token = rig.secrets.github_user().await.unwrap();
    assert_eq!(token.value.expose(), format!("{USER_TOKEN_PREFIX}1"));
    assert!(rig.refreshes().is_empty());
}

#[tokio::test]
async fn the_hourly_check_keeps_a_quiet_sign_in_alive() {
    let rig = Rig::new().await;
    rig.sign_in().await;
    rig.clock.advance(HOUR);
    rig.secrets.keep_github_user().await;
    rig.clock.advance(150 * DAY);
    rig.secrets.keep_github_user().await;
    assert!(rig.refreshes().is_empty());

    // Under 30 days left on the refresh token.
    rig.clock.advance(4 * DAY);
    rig.secrets.keep_github_user().await;
    assert_eq!(rig.refreshes().len(), 1);
    assert_eq!(rig.stored_token(), format!("{USER_TOKEN_PREFIX}2"));
    rig.secrets.keep_github_user().await;
    assert_eq!(rig.refreshes().len(), 1);
}

#[tokio::test]
async fn signing_out_forgets_the_session() {
    let rig = Rig::new().await;
    rig.sign_in().await;
    let id = user_credential(&rig.secrets);
    assert_eq!(rig.secrets.forget_file(id), ActionOutcome::Done);
    assert!(rig.user_file().is_none());
    assert_eq!(signed_in(&rig.secrets), None);
    assert_eq!(
        rig.secrets.github_user().await.unwrap_err(),
        GithubError::NotSignedIn
    );
    assert_eq!(signed_in(&rig.reopen()), None);
    // The App's credential isn't the sign-in's.
    assert_eq!(
        rig.secrets
            .forget_github_user(github_credential(rig.secrets.device())),
        None
    );
}
