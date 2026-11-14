// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the device
// flow's polling, what each answer means, when a session is refreshed and
// what ends it, the check again under the lock, and saving a refreshed pair
// only over the session it came from, rewritten in Rust. The rest is
// MaxHQ's: one device holds the sign-in, the session lives in the store's
// folder, and the hourly check keeps it from lapsing.

//! Your own GitHub sign-in on this device, which merges and approves as
//! you. It's a user token from the GitHub App's device flow, needing only
//! the App's public client ID. The session sits in [`GITHUB_USER_FILE`],
//! and the token never leaves `maxd`: not to a client, a link, a guest or
//! an agent. Each refresh rotates the refresh token, so refreshes take a
//! lock and go one at a time.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use maxhq_protocol::ids::{CredentialId, DeviceId, SignInId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{Credential, CredentialKind};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use serde::{Deserialize, Serialize};
use tokio::task::AbortHandle;
use tracing::{info, warn};
use zeroize::Zeroizing;

use super::api::LoginReply;
use super::{lock, GithubError};
use crate::secrets::{refused, Secrets, GITHUB_USER_FILE};
use crate::session::clock::{Clock, SystemClock};

/// Where a device flow starts.
const DEVICE_CODE: &str = "/login/device/code";
/// Where codes and refresh tokens are traded for tokens.
const ACCESS_TOKEN: &str = "/login/oauth/access_token";
/// The grant a device flow polls with.
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// An access token is refreshed this long before it expires.
const REFRESH_MARGIN: Duration = Duration::from_secs(5 * 60);
/// The hourly check refreshes a session whose refresh token has less than
/// this left, so a sign-in you don't use still lasts.
const KEEP_ALIVE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// Seconds between polls when GitHub names none.
const POLL: u64 = 5;
/// Seconds `slow_down` adds when GitHub names no new interval.
const SLOW_DOWN: u64 = 5;
/// The longest a client ID may be.
const MAX_CLIENT_ID: usize = 100;

/// A code that ran out before you entered it.
pub(super) const EXPIRED: &str = "The sign-in code ran out. Start the sign-in again.";
/// A sign-in you turned down on GitHub.
pub(super) const DENIED: &str = "The sign-in was cancelled on GitHub.";
/// A sign-in this device doesn't know, such as one from before a restart.
pub(super) const GONE: &str = "This sign-in isn't waiting anymore. Start it again.";

/// Your GitHub user token, for merging and approving as you.
#[derive(Clone, Debug)]
pub struct UserToken {
    /// The token, sent as a bearer token.
    pub value: Secret,
    /// Your GitHub login.
    pub login: String,
}

/// The sign-in, and the device flow waiting for you.
pub struct SignIn {
    /// What [`GITHUB_USER_FILE`] holds. Changes to it and to the file go
    /// together under this lock.
    session: Mutex<Option<Session>>,
    /// Held through each refresh.
    refreshing: tokio::sync::Mutex<()>,
    /// The device flow polling for you, if any.
    pending: Mutex<Option<Pending>>,
    /// The last flow that ended, and why if it didn't sign you in. Set
    /// under `pending`'s lock, so a check sees one or the other.
    ended: Mutex<Option<(SignInId, Option<String>)>>,
    /// What polling waits on. Tests bring their own.
    clock: Mutex<Arc<dyn Clock>>,
}

impl Default for SignIn {
    fn default() -> Self {
        Self {
            session: Mutex::new(None),
            refreshing: tokio::sync::Mutex::new(()),
            pending: Mutex::new(None),
            ended: Mutex::new(None),
            clock: Mutex::new(Arc::new(SystemClock)),
        }
    }
}

impl SignIn {
    fn current(&self) -> Option<Session> {
        lock(&self.session).clone()
    }

    fn clock(&self) -> Arc<dyn Clock> {
        lock(&self.clock).clone()
    }

    /// Makes polling wait on `clock`.
    #[cfg(test)]
    pub(in crate::secrets) fn set_clock(&self, clock: Arc<dyn Clock>) {
        *lock(&self.clock) = clock;
    }

    /// Marks `rejected` refused, so the next ask refreshes. A token that
    /// isn't the session's says nothing about it.
    fn refuse(&self, rejected: &Secret) {
        if let Some(session) = lock(&self.session)
            .as_mut()
            .filter(|session| session.access_token == *rejected)
        {
            session.refused = true;
        }
    }
}

/// A device flow polling for you.
struct Pending {
    id: SignInId,
    task: AbortHandle,
}

/// [`GITHUB_USER_FILE`].
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Session {
    access_token: Secret,
    /// None when the App hands out tokens that don't expire.
    #[serde(default)]
    access_expires_at: Option<Timestamp>,
    #[serde(default)]
    refresh_token: Option<Secret>,
    #[serde(default)]
    refresh_expires_at: Option<Timestamp>,
    login: String,
    user_id: u64,
    client_id: String,
    signed_in_at: Timestamp,
    /// GitHub refused the access token, so it's refreshed before reuse.
    #[serde(skip)]
    refused: bool,
}

impl Session {
    /// The session `reply` starts, or the one a refresh of `self` gives.
    fn from_reply(
        reply: LoginReply,
        access_token: Secret,
        now: Timestamp,
        login: String,
        user_id: u64,
        client_id: String,
        signed_in_at: Timestamp,
    ) -> Self {
        let after = |secs: Option<u64>| secs.map(|secs| now.after(Duration::from_secs(secs)));
        Self {
            access_token,
            access_expires_at: after(reply.expires_in),
            refresh_token: reply.refresh_token,
            refresh_expires_at: after(reply.refresh_token_expires_in),
            login,
            user_id,
            client_id,
            signed_in_at,
            refused: false,
        }
    }

    /// Whether the access token serves as is: not refused and with more
    /// than [`REFRESH_MARGIN`] left.
    fn fresh(&self, now: Timestamp) -> bool {
        !self.refused
            && self
                .access_expires_at
                .is_none_or(|at| now.after(REFRESH_MARGIN) < at)
    }

    /// Whether the access token still works, if only just.
    fn usable(&self, now: Timestamp) -> bool {
        !self.refused && self.access_expires_at.is_none_or(|at| now < at)
    }

    /// Whether the refresh token lapses within [`KEEP_ALIVE`].
    fn lapsing(&self, now: Timestamp) -> bool {
        self.refresh_token.is_some()
            && self
                .refresh_expires_at
                .is_some_and(|at| now.after(KEEP_ALIVE) > at)
    }

    fn token(&self) -> UserToken {
        UserToken {
            value: self.access_token.clone(),
            login: self.login.clone(),
        }
    }
}

/// What one poll of the device flow comes to.
#[derive(Debug, PartialEq)]
pub(super) enum Polled {
    /// You signed in.
    Token,
    /// Poll again after this many seconds.
    Wait(u64),
    /// The flow ended, for this reason.
    Failed(String),
}

/// What `reply` to a poll means, polling every `interval` seconds.
pub(super) fn polled(reply: &LoginReply, interval: u64) -> Polled {
    if reply.access_token.is_some() {
        return Polled::Token;
    }
    match reply.error.as_deref() {
        Some("authorization_pending") => Polled::Wait(interval),
        Some("slow_down") => Polled::Wait(reply.interval.unwrap_or(interval + SLOW_DOWN)),
        Some("expired_token") => Polled::Failed(EXPIRED.to_owned()),
        Some("access_denied") => Polled::Failed(DENIED.to_owned()),
        _ => Polled::Failed(format!("GitHub refused the sign-in: {}", said(reply))),
    }
}

/// GitHub's words for a refusal in `reply`, kept short.
fn said(reply: &LoginReply) -> String {
    let text = reply
        .error_description
        .as_deref()
        .or(reply.error.as_deref())
        .unwrap_or("no reason given");
    text.chars().take(300).collect()
}

/// Whether `client_id` reads as a GitHub App's client ID.
fn client_id_ok(client_id: &str) -> bool {
    (1..=MAX_CLIENT_ID).contains(&client_id.len())
        && client_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.')
}

/// Your sign-in's credential on `device`.
fn user_credential(device: DeviceId) -> CredentialId {
    CredentialId::derive(&format!("github-user:{device}"))
}

impl Secrets {
    fn user(&self) -> &SignIn {
        &self.inner.github.user
    }

    /// Reads the sign-in the folder holds, if any, and publishes it.
    pub(in crate::secrets) fn load_github_user(&self) {
        let text = match self.inner.folder.read(GITHUB_USER_FILE) {
            Ok(Some(text)) => text,
            Ok(None) => return,
            Err(err) => {
                warn!("skipped {GITHUB_USER_FILE}: {err}");
                return;
            }
        };
        let Ok(session) = serde_json::from_str::<Session>(text.expose()) else {
            warn!("skipped {GITHUB_USER_FILE}: it doesn't hold a GitHub sign-in");
            return;
        };
        self.publish_github_user(&session);
        *lock(&self.user().session) = Some(session);
    }

    /// Your user token, refreshed first when it has under five minutes
    /// left or GitHub refused it. [`GithubError::NotSignedIn`] while no
    /// one has signed in on this device, or once GitHub ended the sign-in.
    pub async fn github_user(&self) -> Result<UserToken, GithubError> {
        let user = self.user();
        let session = user.current().ok_or(GithubError::NotSignedIn)?;
        if session.fresh(self.now()) {
            return Ok(session.token());
        }
        let _refreshing = user.refreshing.lock().await;
        // Another ask may have refreshed while this one waited.
        let session = user.current().ok_or(GithubError::NotSignedIn)?;
        let now = self.now();
        if session.fresh(now) {
            return Ok(session.token());
        }
        match self.refresh_github_user(&session).await {
            Ok(token) => token.ok_or(GithubError::NotSignedIn),
            Err(err) if session.usable(now) => {
                // A refresh ahead of expiry that fails leaves the token
                // that still works in use.
                warn!("couldn't refresh the GitHub sign-in, so kept its token: {err}");
                Ok(session.token())
            }
            Err(err) => {
                warn!("couldn't refresh the GitHub sign-in: {err}");
                Err(err)
            }
        }
    }

    /// Runs `act` with your user token. When GitHub refuses the token with
    /// 401, it's refreshed and `act` runs once more.
    pub async fn with_github_user<T, F, Fut>(&self, act: F) -> Result<T, GithubError>
    where
        F: Fn(UserToken) -> Fut,
        Fut: Future<Output = Result<T, GithubError>>,
    {
        let token = self.github_user().await?;
        self.again_if_refused(token, act).await
    }

    /// Runs `act` with `token`, and once more with a fresh token when GitHub
    /// refuses `token` with 401.
    pub(super) async fn again_if_refused<T, F, Fut>(
        &self,
        token: UserToken,
        act: F,
    ) -> Result<T, GithubError>
    where
        F: Fn(UserToken) -> Fut,
        Fut: Future<Output = Result<T, GithubError>>,
    {
        let rejected = token.value.clone();
        match act(token).await {
            Err(GithubError::Refused { status: 401, .. }) => {
                self.user().refuse(&rejected);
                act(self.github_user().await?).await
            }
            done => done,
        }
    }

    /// Refreshes `from`, which needs the lock held. None when GitHub ended
    /// the sign-in, which signs out, or when the sign-in changed meanwhile
    /// and has no session now.
    async fn refresh_github_user(&self, from: &Session) -> Result<Option<UserToken>, GithubError> {
        let now = self.now();
        let refresh_token = from
            .refresh_token
            .clone()
            .filter(|_| from.refresh_expires_at.is_none_or(|at| now < at));
        let Some(refresh_token) = refresh_token else {
            warn!("the GitHub sign-in can't be refreshed, so it ended");
            self.replace_github_user(from, None).await;
            return Ok(None);
        };
        let form = [
            ("client_id", from.client_id.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.expose()),
        ];
        let mut reply = self.inner.github.api.login(ACCESS_TOKEN, &form).await?;
        if reply.error.as_deref() == Some("bad_refresh_token") {
            // You revoked the App, or the session lapsed.
            warn!("GitHub ended the GitHub sign-in");
            self.replace_github_user(from, None).await;
            return Ok(None);
        }
        let Some(access_token) = reply.access_token.take() else {
            return Err(GithubError::Refused {
                status: 200,
                message: said(&reply),
            });
        };
        let next = Session::from_reply(
            reply,
            access_token,
            now,
            from.login.clone(),
            from.user_id,
            from.client_id.clone(),
            from.signed_in_at,
        );
        if self.replace_github_user(from, Some(next.clone())).await {
            info!(login = %next.login, "refreshed the GitHub sign-in");
            return Ok(Some(next.token()));
        }
        // Signed out or in again meanwhile, so whatever is there now.
        Ok(self.user().current().map(|session| session.token()))
    }

    /// Puts `next` in place of `from`, in the file and in memory, unless
    /// the sign-in changed since `from` was read. None signs out. Returns
    /// whether it was put in place.
    async fn replace_github_user(&self, from: &Session, next: Option<Session>) -> bool {
        let secrets = self.clone();
        let from = from.access_token.clone();
        tokio::task::spawn_blocking(move || {
            let mut session = lock(&secrets.user().session);
            if session.as_ref().map(|session| &session.access_token) != Some(&from) {
                return false;
            }
            match &next {
                Some(next) => {
                    // GitHub has retired the old pair, so the new one is
                    // used even when the file can't take it.
                    if let Err(err) = secrets.write_github_user(next) {
                        warn!("couldn't store the refreshed GitHub sign-in: {err}");
                    }
                }
                None => {
                    if let Err(err) = secrets.inner.folder.remove(GITHUB_USER_FILE) {
                        warn!("couldn't delete {GITHUB_USER_FILE}: {err}");
                    }
                    secrets.unpublish_github_user();
                }
            }
            *session = next;
            true
        })
        .await
        .unwrap_or(false)
    }

    fn write_github_user(&self, session: &Session) -> std::io::Result<()> {
        let text = serde_json::to_vec_pretty(session).map(Zeroizing::new)?;
        self.inner.folder.write(GITHUB_USER_FILE, &text)
    }

    /// Refreshes the sign-in when its refresh token lapses within
    /// [`KEEP_ALIVE`]. The hourly check calls it.
    pub(in crate::secrets) async fn keep_github_user(&self) {
        let user = self.user();
        if !user.current().is_some_and(|s| s.lapsing(self.now())) {
            return;
        }
        let _refreshing = user.refreshing.lock().await;
        let Some(session) = user.current().filter(|s| s.lapsing(self.now())) else {
            return;
        };
        if let Err(err) = self.refresh_github_user(&session).await {
            warn!("couldn't refresh the GitHub sign-in before it lapses: {err}");
        }
    }

    /// Answers `BeginGithubSignIn`: starts the App's device flow, through
    /// this device's App or else the client ID the app passed, and polls
    /// for you in the background. A newer sign-in replaces one still
    /// waiting.
    pub(in crate::secrets) async fn begin_github_user(
        &self,
        client_id: Option<String>,
    ) -> ActionOutcome {
        let app = self.inner.github.current();
        let (own, slug) = app.map_or((None, None), |app| {
            let found = lock(&app.found);
            (found.client_id.clone(), found.slug.clone())
        });
        let passed = client_id.map(|id| id.trim().to_owned());
        let Some(client_id) = own.or(passed).filter(|id| client_id_ok(id)) else {
            return refused(
                "This device has no GitHub App to sign in through. Set one up here, or sign in from a device that has one.",
            );
        };
        let api = &self.inner.github.api;
        let reply = match api.login(DEVICE_CODE, &[("client_id", &client_id)]).await {
            Ok(reply) => reply,
            Err(err) => {
                warn!("couldn't start a GitHub sign-in: {err}");
                return refused(format!("GitHub wouldn't start the sign-in. {err}"));
            }
        };
        let LoginReply {
            device_code: Some(device_code),
            user_code: Some(user_code),
            verification_uri: Some(verification_uri),
            expires_in: Some(expires_in),
            interval,
            ..
        } = reply
        else {
            if reply.error.as_deref() == Some("device_flow_disabled") {
                let at = slug
                    .map(|slug| {
                        format!(" Its settings are at https://github.com/settings/apps/{slug}.")
                    })
                    .unwrap_or_default();
                return refused(format!(
                    "Device Flow is off for the GitHub App. In the App's settings on GitHub, under \"Identifying and authorizing users\", tick Enable Device Flow and save. Then sign in again.{at}"
                ));
            }
            return refused(format!(
                "GitHub wouldn't start the sign-in: {}",
                said(&reply)
            ));
        };

        let sign_in = SignInId::generate();
        let expires_at = self.now().after(Duration::from_secs(expires_in));
        let secrets = self.clone();
        let mut pending = lock(&self.user().pending);
        let task = tokio::spawn(async move {
            let polled = secrets
                .poll_github_user(&client_id, &device_code, expires_in, interval)
                .await;
            secrets.finish_github_user(sign_in, polled).await;
        });
        let replaced = pending.replace(Pending {
            id: sign_in,
            task: task.abort_handle(),
        });
        drop(pending);
        if let Some(replaced) = replaced {
            replaced.task.abort();
            info!(sign_in = %replaced.id, "a newer GitHub sign-in replaced this one");
        }
        info!(%sign_in, "started a GitHub sign-in");
        ActionOutcome::DeviceCode {
            sign_in,
            user_code,
            verification_uri,
            expires_at,
        }
    }

    /// Answers `CancelGithubSignIn`: stops polling for `sign_in`. One that
    /// already ended counts as stopped.
    pub(in crate::secrets) fn cancel_github_user(&self, sign_in: SignInId) -> ActionOutcome {
        let mut pending = lock(&self.user().pending);
        if let Some(pending) = pending.take_if(|pending| pending.id == sign_in) {
            pending.task.abort();
            info!(%sign_in, "cancelled the GitHub sign-in");
        }
        ActionOutcome::Done
    }

    /// Answers `CheckGithubSignIn`: done while `sign_in` waits and once
    /// it signed you in, refused with why once it ended otherwise.
    pub(in crate::secrets) fn check_github_user(&self, sign_in: SignInId) -> ActionOutcome {
        let pending = lock(&self.user().pending);
        if pending
            .as_ref()
            .is_some_and(|pending| pending.id == sign_in)
        {
            return ActionOutcome::Done;
        }
        match &*lock(&self.user().ended) {
            Some((ended, None)) if *ended == sign_in => ActionOutcome::Done,
            Some((ended, Some(why))) if *ended == sign_in => refused(why.clone()),
            _ => refused(GONE),
        }
    }

    /// Polls GitHub with `device_code` until you authorize the App, then
    /// asks who you are. The error says why the flow ended.
    pub(super) async fn poll_github_user(
        &self,
        client_id: &str,
        device_code: &Secret,
        expires_in: u64,
        interval: Option<u64>,
    ) -> Result<Session, String> {
        let api = &self.inner.github.api;
        let clock = self.user().clock();
        let deadline = clock.now() + Duration::from_secs(expires_in);
        let mut interval = interval.unwrap_or(POLL);
        let form = [
            ("client_id", client_id),
            ("device_code", device_code.expose()),
            ("grant_type", DEVICE_GRANT),
        ];
        let mut reply = loop {
            clock
                .sleep_until(clock.now() + Duration::from_secs(interval))
                .await;
            if clock.now() >= deadline {
                return Err(EXPIRED.to_owned());
            }
            // A dropped poll is tried again on the next one, since the
            // code works until it expires.
            let Ok(reply) = api.login(ACCESS_TOKEN, &form).await else {
                continue;
            };
            match polled(&reply, interval) {
                Polled::Token => break reply,
                Polled::Wait(next) => interval = next,
                Polled::Failed(why) => return Err(why),
            }
        };
        let Some(access_token) = reply.access_token.take() else {
            return Err("GitHub's reply wasn't what this device expected.".to_owned());
        };
        let you = api
            .user(&access_token)
            .await
            .map_err(|err| format!("GitHub signed you in but wouldn't say who you are. {err}"))?;
        let now = self.now();
        Ok(Session::from_reply(
            reply,
            access_token,
            now,
            you.login,
            you.id,
            client_id.to_owned(),
            now,
        ))
    }

    /// Stores and publishes the session `polled` gave, unless a newer
    /// sign-in or a cancel took its place meanwhile, and keeps how it
    /// ended for `CheckGithubSignIn`.
    async fn finish_github_user(&self, sign_in: SignInId, polled: Result<Session, String>) {
        let secrets = self.clone();
        // Logged here rather than on the blocking thread, so the log stays
        // with the task that polled.
        let stored = tokio::task::spawn_blocking(move || -> Result<Option<String>, String> {
            let mut pending = lock(&secrets.user().pending);
            if pending.as_ref().is_none_or(|pending| pending.id != sign_in) {
                return Ok(None);
            }
            *pending = None;
            let stored = polled.and_then(|session| {
                let mut current = lock(&secrets.user().session);
                secrets
                    .write_github_user(&session)
                    .map_err(|err| format!("This device couldn't store the sign-in: {err}"))?;
                secrets.publish_github_user(&session);
                let login = session.login.clone();
                *current = Some(session);
                Ok(login)
            });
            *lock(&secrets.user().ended) = Some((sign_in, stored.as_ref().err().cloned()));
            stored.map(Some)
        })
        .await;
        match stored {
            Ok(Ok(Some(login))) => info!(%sign_in, %login, "signed in to GitHub"),
            Ok(Ok(None)) => {}
            Ok(Err(why)) => warn!(%sign_in, "the GitHub sign-in ended: {why}"),
            Err(err) => warn!(%sign_in, "the GitHub sign-in ended: {err}"),
        }
    }

    /// Signs out: deletes the session and takes its credential away.
    /// Returns none when `credential` isn't the sign-in's.
    pub(in crate::secrets) fn forget_github_user(
        &self,
        credential: CredentialId,
    ) -> Option<ActionOutcome> {
        if credential != user_credential(self.device()) {
            return None;
        }
        let mut session = lock(&self.user().session);
        let login = session.as_ref()?.login.clone();
        if let Err(err) = self.inner.folder.remove(GITHUB_USER_FILE) {
            warn!("couldn't delete {GITHUB_USER_FILE}: {err}");
            return Some(refused(format!(
                "This device couldn't delete the sign-in: {err}"
            )));
        }
        *session = None;
        self.unpublish_github_user();
        info!(%login, "signed out of GitHub");
        Some(ActionOutcome::Done)
    }

    fn publish_github_user(&self, session: &Session) {
        let device = self.device();
        self.inner.hub.apply([Change::Upsert {
            value: Entity::Credential(Credential {
                id: user_credential(device),
                device,
                kind: CredentialKind::GithubUser {
                    login: session.login.clone(),
                },
                stored_at: session.signed_in_at,
                problem: None,
            }),
        }]);
    }

    fn unpublish_github_user(&self) {
        self.inner.hub.apply([Change::Remove {
            key: EntityKey::Credential(user_credential(self.device())),
        }]);
    }
}
