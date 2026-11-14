// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the sign-in
// round's parameters, the checks on its callback and on the account it
// signs in, when a session is renewed and what ends it, and sign-out's
// revocation, translated to Rust. The rest is MaxHQ's: the round runs on
// the device while the browser and its callback run on the Mac, and the
// store and the token broker are maxd's.

//! Sign in with ChatGPT, per W-5, for Codex agents in this device's
//! containers. The device makes its own host ID once, before its first
//! sign-in, and keeps it for good. It starts each round itself, keeping
//! the PKCE verifier, the state and the nonce, so the Mac's browser hands
//! back only the code. The device trades the code for a session, checks
//! the identity token and the plan-usage scope, keeps the session in
//! [`CHATGPT_FILE`], and owns every renewal from then on. Renewals take
//! the session's lock, since each one rotates the refresh token.
//!
//! Codex in a container asks for an access token through `maxd guest
//! token codex`, and the guest link brings the ask here. Sign-out revokes
//! the refresh token, then clears the session, and keeps the host ID and
//! the client registration for the next sign-in.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use maxhq_core::names;
use maxhq_protocol::guest::TokenReply;
use maxhq_protocol::ids::{AccountId, CredentialId, DeviceId, SignInId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{Account, Credential, CredentialKind, LoginKind, Vendor};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use ring::digest::{digest, SHA256};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, warn};
use url::Url;
use zeroize::Zeroizing;

use self::oauth::{Grant, TokenError, Tokens, RESOURCE};
pub use self::oauth::{Issuer, OPENAI_ISSUER};
use super::{files, login_account, refused, Secrets, CHATGPT_FILE};
use crate::observe::claude::time::parse_iso8601;
use crate::recipe::accounts::codex_folder;

#[cfg(test)]
pub mod fake;
mod id_token;
mod oauth;
#[cfg(test)]
mod tests;

/// The client a first sign-in registers through.
const NEW_CLIENT: &str = "dynamic_agent_client";
/// The name a first sign-in suggests for the registered client.
const AGENT_NAME: &str = "MaxHQ";
/// What a sign-in asks for.
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// The scope that lets Codex run on the ChatGPT plan.
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
/// How long a round waits for its callback, as long as the app listens.
const ROUND_LIMIT: Duration = Duration::from_secs(10 * 60);
/// The most rounds waiting at once. A new one drops the oldest.
const MAX_ROUNDS: usize = 4;
/// An access token is renewed once it has less than this left.
const RENEW_MARGIN_MS: u64 = 60_000;
/// The callback port is picked from the dynamic range.
const FIRST_PORT: u16 = 49_152;

/// Why a callback can't finish a round, said the same way each time.
const INCOMPLETE: &str = "The Codex sign-in didn't finish registering this device. Start it again.";
/// A sign-in without the plan-usage scope.
const NO_PLAN: &str = "Signed in, but plan usage wasn't allowed, so Codex can't run on this sign-in. Sign in again and allow it.";
/// A session that can't be renewed.
const ENDED: &str = "The Codex sign-in on this device ended. Sign in again.";

/// The sign-in and the rounds waiting for their callback.
pub struct ChatGpt {
    issuer: Issuer,
    /// What [`CHATGPT_FILE`] holds. Each renewal holds this lock, so a
    /// session's renewals go one at a time.
    saved: tokio::sync::Mutex<Saved>,
    rounds: Mutex<HashMap<SignInId, Round>>,
    /// Whether there's a session with plan usage, for checks that can't
    /// wait on the lock.
    usable: AtomicBool,
}

/// [`CHATGPT_FILE`].
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    /// This device's `ext_agent_host_id`, made once.
    #[serde(default)]
    host_id: Option<String>,
    /// The client this device registered, kept across sign-outs.
    #[serde(default)]
    registration: Option<Registration>,
    /// The session, while signed in.
    #[serde(default)]
    session: Option<Session>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Registration {
    client_id: String,
    /// The account it signed in, once one has.
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Session {
    client_id: String,
    subject: String,
    email: Option<String>,
    access_token: Secret,
    refresh_token: Secret,
    expires_at: Timestamp,
    /// The soonest OpenAI takes a renewal.
    earliest_refresh_at: Option<Timestamp>,
    scopes: Vec<String>,
    stored_at: Timestamp,
}

impl Session {
    fn has_plan(&self) -> bool {
        self.scopes.iter().any(|scope| scope == PLAN_SCOPE)
    }
}

/// A round waiting for its callback.
struct Round {
    state: String,
    nonce: Secret,
    verifier: Secret,
    redirect_uri: String,
    /// The client it signs in through, when the device has registered one.
    registered: Option<String>,
    until: Instant,
}

impl ChatGpt {
    /// The sign-in `folder` holds, against `issuer`.
    pub fn open(issuer: Issuer, folder: &files::Folder) -> Self {
        let saved = match folder.read(CHATGPT_FILE) {
            Ok(Some(text)) => serde_json::from_str(text.expose()).unwrap_or_else(|_| {
                warn!("skipped {CHATGPT_FILE}: it doesn't hold a ChatGPT sign-in");
                Saved::default()
            }),
            Ok(None) => Saved::default(),
            Err(err) => {
                warn!("skipped {CHATGPT_FILE}: {err}");
                Saved::default()
            }
        };
        let usable = saved.session.as_ref().is_some_and(Session::has_plan);
        Self {
            issuer,
            saved: tokio::sync::Mutex::new(saved),
            rounds: Mutex::new(HashMap::new()),
            usable: AtomicBool::new(usable),
        }
    }

    fn rounds(&self) -> MutexGuard<'_, HashMap<SignInId, Round>> {
        self.rounds.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The Codex account the sign-in gives `device`.
pub(super) fn chatgpt_account(device: DeviceId) -> AccountId {
    names::account(device, Vendor::Codex, "chatgpt")
}

/// The credential for the sign-in on `device`.
fn credential_id(device: DeviceId) -> CredentialId {
    CredentialId::derive(&format!("chatgpt:{device}"))
}

/// Whether `client_id` is one OpenAI issued.
fn issued(client_id: &str) -> bool {
    client_id.strip_prefix("oaiapp_").is_some_and(|rest| {
        !rest.is_empty()
            && rest.len() <= 200
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

/// `len` random bytes in base64url.
fn random(rng: &SystemRandom, len: usize) -> Option<String> {
    let mut bytes = Zeroizing::new(vec![0; len]);
    rng.fill(&mut bytes).ok()?;
    Some(URL_SAFE_NO_PAD.encode(&*bytes))
}

/// When `earliest_refresh_at` says, in Unix seconds or ISO 8601.
fn earliest(value: Option<&Value>) -> Option<Timestamp> {
    match value? {
        Value::Number(secs) => secs
            .as_u64()
            .map(|secs| Timestamp(secs.saturating_mul(1000))),
        Value::String(text) => parse_iso8601(text),
        _ => None,
    }
}

fn scopes(tokens: &Tokens) -> Vec<String> {
    tokens.scope.split_whitespace().map(str::to_owned).collect()
}

fn refuse(reason: &str) -> TokenReply {
    TokenReply::Refused {
        reason: reason.to_owned(),
    }
}

impl Secrets {
    fn chatgpt(&self) -> &ChatGpt {
        &self.inner.chatgpt
    }

    /// Whether `account` is this device's sign-in, with plan usage.
    pub(super) fn has_signed_in(&self, account: AccountId) -> bool {
        account == chatgpt_account(self.device()) && self.chatgpt().usable.load(Ordering::SeqCst)
    }

    /// Writes `saved` to [`CHATGPT_FILE`] off the async threads.
    async fn persist(&self, saved: &Saved) -> io::Result<()> {
        let text = serde_json::to_vec(saved).map(Zeroizing::new)?;
        let secrets = self.clone();
        tokio::task::spawn_blocking(move || secrets.inner.folder.write(CHATGPT_FILE, &text))
            .await
            .map_err(io::Error::other)?
    }

    /// Publishes the sign-in, if there is one.
    pub(super) fn load_chatgpt(&self) {
        let Ok(saved) = self.chatgpt().saved.try_lock() else {
            return;
        };
        if let Some(session) = &saved.session {
            self.publish_session(session);
        }
    }

    /// Answers `BeginChatGptSignIn`: makes the host ID if this is the
    /// device's first round, and gives the page to open and the port the
    /// browser comes back to.
    pub(super) async fn begin_chatgpt(&self) -> ActionOutcome {
        let chatgpt = self.chatgpt();
        let endpoints = match chatgpt.issuer.endpoints().await {
            Ok(endpoints) => endpoints,
            Err(why) => {
                warn!("couldn't read OpenAI's sign-in server: {why}");
                return refused("OpenAI's sign-in server couldn't be reached. Try again.");
            }
        };
        let rng = SystemRandom::new();
        let mut port = [0; 2];
        let (Some(state), Some(nonce), Some(verifier), Ok(())) = (
            random(&rng, 32),
            random(&rng, 32),
            random(&rng, 64),
            rng.fill(&mut port),
        ) else {
            return refused("This device couldn't start a sign-in.");
        };
        let port = FIRST_PORT + u16::from_be_bytes(port) % (u16::MAX - FIRST_PORT + 1);
        let redirect_uri = format!("http://127.0.0.1:{port}/auth/callback");
        let challenge = URL_SAFE_NO_PAD.encode(digest(&SHA256, verifier.as_bytes()));

        let mut saved = chatgpt.saved.lock().await;
        let host_id = match &saved.host_id {
            Some(host_id) => host_id.clone(),
            None => {
                let mut bytes = [0; 16];
                if rng.fill(&mut bytes).is_err() {
                    return refused("This device couldn't start a sign-in.");
                }
                let host_id = format!(
                    "urn:uuid:{}",
                    uuid::Builder::from_random_bytes(bytes).into_uuid()
                );
                saved.host_id = Some(host_id.clone());
                if let Err(err) = self.persist(&saved).await {
                    saved.host_id = None;
                    warn!("couldn't store this device's ChatGPT host ID: {err}");
                    return refused(format!("This device couldn't start a sign-in: {err}"));
                }
                info!("made this device's ChatGPT host ID");
                host_id
            }
        };
        let Ok(mut url) = Url::parse(&endpoints.authorization_endpoint) else {
            return refused("OpenAI's sign-in server gave a page that doesn't read.");
        };
        let registered = saved.registration.as_ref().map(|r| r.client_id.clone());
        {
            let mut query = url.query_pairs_mut();
            match &saved.registration {
                Some(registration) => {
                    query.append_pair("client_id", &registration.client_id);
                    if let Some(email) = &registration.email {
                        query.append_pair("login_hint", email);
                    }
                    if saved.session.as_ref().is_some_and(|s| !s.has_plan()) {
                        query.append_pair("prompt", "consent");
                    }
                }
                None => {
                    query
                        .append_pair("client_id", NEW_CLIENT)
                        .append_pair("agent_name_hint", AGENT_NAME);
                }
            }
            query
                .append_pair("ext_agent_host_id", &host_id)
                .append_pair("response_type", "code")
                .append_pair("redirect_uri", &redirect_uri)
                .append_pair("scope", SCOPES)
                .append_pair("resource", RESOURCE)
                .append_pair("state", &state)
                .append_pair("nonce", &nonce)
                .append_pair("code_challenge_method", "S256")
                .append_pair("code_challenge", &challenge);
        }
        drop(saved);

        let sign_in = SignInId::generate();
        let now = Instant::now();
        let mut rounds = chatgpt.rounds();
        rounds.retain(|_, round| round.until > now);
        while rounds.len() >= MAX_ROUNDS {
            let oldest = rounds
                .iter()
                .min_by_key(|(_, round)| round.until)
                .map(|(id, _)| *id);
            oldest.map(|id| rounds.remove(&id));
        }
        rounds.insert(
            sign_in,
            Round {
                state,
                nonce: Secret::new(nonce),
                verifier: Secret::new(verifier),
                redirect_uri,
                registered,
                until: now + ROUND_LIMIT,
            },
        );
        info!(%sign_in, "started a ChatGPT sign-in");
        ActionOutcome::SignIn {
            sign_in,
            url: url.into(),
            callback_port: port,
        }
    }

    /// Answers `FinishChatGptSignIn`: trades the code for a session,
    /// checks who signed in and what they allowed, and stores it.
    pub(super) async fn finish_chatgpt(
        &self,
        sign_in: SignInId,
        code: Secret,
        state: String,
        client_id: Option<String>,
    ) -> ActionOutcome {
        let chatgpt = self.chatgpt();
        let round = chatgpt.rounds().remove(&sign_in);
        let Some(round) = round.filter(|round| round.until > Instant::now()) else {
            return refused("That sign-in ended. Start it again.");
        };
        if round.state != state {
            return refused("The sign-in couldn't be verified. Start it again.");
        }
        let client_id = match (&round.registered, client_id) {
            (Some(registered), Some(got)) if *registered != got => return refused(INCOMPLETE),
            (Some(registered), _) => registered.clone(),
            (None, Some(got)) if issued(&got) => got,
            (None, _) => return refused(INCOMPLETE),
        };
        let endpoints = match chatgpt.issuer.endpoints().await {
            Ok(endpoints) => endpoints,
            Err(why) => {
                warn!("couldn't read OpenAI's sign-in server: {why}");
                return refused("OpenAI's sign-in server couldn't be reached. Try again.");
            }
        };
        let grant = Grant::Code {
            client_id: &client_id,
            code: &code,
            verifier: &round.verifier,
            redirect_uri: &round.redirect_uri,
        };
        let tokens = match chatgpt.issuer.tokens(&endpoints, grant).await {
            Ok(tokens) => tokens,
            Err(err) => return self.failed_exchange(err, &round, client_id).await,
        };
        let (Some(id_token), Some(refresh_token)) = (&tokens.id_token, &tokens.refresh_token)
        else {
            return refused("OpenAI didn't return a lasting session. Sign in again.");
        };
        if !tokens.token_type.eq_ignore_ascii_case("bearer") {
            return refused("OpenAI returned a session Codex can't use. Sign in again.");
        }
        let keys = match chatgpt.issuer.keys(&endpoints).await {
            Ok(keys) => keys,
            Err(why) => {
                warn!("couldn't read OpenAI's signing keys: {why}");
                return refused("OpenAI's sign-in server couldn't be reached. Try again.");
            }
        };
        let expected = id_token::Expected {
            issuer: &endpoints.issuer,
            audience: &client_id,
            nonce: &round.nonce,
            now: self.now(),
        };
        let identity = match id_token::verify(id_token, &keys, &expected) {
            Ok(identity) => identity,
            Err(why) => {
                warn!("a ChatGPT sign-in's identity token failed its check: {why}");
                return refused("The sign-in couldn't be verified. Start it again.");
            }
        };

        let mut saved = chatgpt.saved.lock().await;
        let known = saved
            .registration
            .as_ref()
            .filter(|registration| registration.client_id == client_id)
            .and_then(|registration| registration.subject.as_deref());
        if known.is_some_and(|subject| subject != identity.subject) {
            if saved.session.is_some() {
                return refused(
                    "This sign-in returned a different account. Sign out on this device first to switch accounts.",
                );
            }
            saved.registration = None;
            if let Err(err) = self.persist(&saved).await {
                warn!("couldn't store the ChatGPT sign-in: {err}");
            }
            return refused(
                "This sign-in returned a different account. Sign in again to connect it.",
            );
        }
        let now = self.now();
        let session = Session {
            client_id: client_id.clone(),
            subject: identity.subject.clone(),
            email: identity.email.clone(),
            access_token: tokens.access_token.clone(),
            refresh_token: refresh_token.clone(),
            expires_at: now.after(Duration::from_secs(tokens.expires_in)),
            earliest_refresh_at: earliest(tokens.earliest_refresh_at.as_ref()),
            scopes: scopes(&tokens),
            stored_at: now,
        };
        let previous = saved.registration.replace(Registration {
            client_id,
            subject: Some(identity.subject),
            email: identity.email,
        });
        let replaced = saved.session.replace(session.clone());
        if let Err(err) = self.persist(&saved).await {
            saved.registration = previous;
            saved.session = replaced;
            warn!("couldn't store the ChatGPT sign-in: {err}");
            return refused(format!("This device couldn't store the sign-in: {err}"));
        }
        drop(saved);
        self.publish_session(&session);
        info!(%sign_in, plan = session.has_plan(), "stored a ChatGPT sign-in");
        if session.has_plan() {
            ActionOutcome::Done
        } else {
            refused(NO_PLAN)
        }
    }

    /// What a code the token endpoint refused comes to.
    async fn failed_exchange(
        &self,
        err: TokenError,
        round: &Round,
        client_id: String,
    ) -> ActionOutcome {
        warn!("a ChatGPT sign-in's code was refused: {err}");
        match err {
            TokenError::CodeExpired => {
                // The client was registered all the same, so the next
                // round signs in through it.
                if round.registered.is_none() {
                    let mut saved = self.chatgpt().saved.lock().await;
                    if saved.registration.is_none() {
                        saved.registration = Some(Registration {
                            client_id,
                            subject: None,
                            email: None,
                        });
                        if let Err(err) = self.persist(&saved).await {
                            warn!("couldn't store the ChatGPT registration: {err}");
                        }
                    }
                }
                refused("This sign-in's code ran out. Start it again.")
            }
            TokenError::Declined => {
                refused("The sign-in was declined. Sign in again when you're ready.")
            }
            TokenError::BadClient => {
                // A new round registers the device again.
                let mut saved = self.chatgpt().saved.lock().await;
                if saved.session.is_none()
                    && saved.registration.take().is_some()
                    && let Err(err) = self.persist(&saved).await
                {
                    warn!("couldn't store the ChatGPT registration: {err}");
                }
                refused("OpenAI refused this device's registration. Sign in again.")
            }
            TokenError::Unreachable(_) => {
                refused("OpenAI's sign-in server couldn't be reached. Try again.")
            }
            _ => refused("OpenAI couldn't finish the sign-in. Try again."),
        }
    }

    /// An access token for Codex on `account`, renewed first when it has
    /// under a minute left.
    pub(super) async fn chatgpt_token(&self, account: AccountId) -> TokenReply {
        if account != chatgpt_account(self.device()) {
            return refuse("this device has no Codex sign-in for that account");
        }
        let chatgpt = self.chatgpt();
        let mut saved = chatgpt.saved.lock().await;
        let Some(session) = &saved.session else {
            return refuse("this device isn't signed in to Codex");
        };
        if !session.has_plan() {
            return refuse(NO_PLAN);
        }
        let now = self.now();
        let current = TokenReply::Token {
            value: session.access_token.clone(),
            expires_at: session.expires_at,
        };
        if session.expires_at.0 > now.0.saturating_add(RENEW_MARGIN_MS) {
            return current;
        }
        if session.earliest_refresh_at.is_some_and(|at| at > now) {
            return if session.expires_at > now {
                current
            } else {
                refuse("the Codex sign-in can't be renewed yet, so try again shortly")
            };
        }
        let endpoints = match chatgpt.issuer.endpoints().await {
            Ok(endpoints) => endpoints,
            Err(why) => {
                warn!("couldn't read OpenAI's sign-in server: {why}");
                return refuse("OpenAI's sign-in server couldn't be reached");
            }
        };
        let grant = Grant::Refresh {
            client_id: &session.client_id,
            refresh_token: &session.refresh_token,
        };
        let tokens = match chatgpt.issuer.tokens(&endpoints, grant).await {
            Ok(tokens) => tokens,
            Err(TokenError::Ended) => {
                warn!("the ChatGPT session ended");
                saved.session = None;
                chatgpt.usable.store(false, Ordering::SeqCst);
                if let Err(err) = self.persist(&saved).await {
                    warn!("couldn't clear the ChatGPT session: {err}");
                }
                self.unpublish_session();
                return refuse(ENDED);
            }
            Err(TokenError::BadClient) => {
                warn!("OpenAI refused this device's ChatGPT registration");
                return refuse("OpenAI refused this device's registration, so sign in again");
            }
            Err(err) => {
                // A server that's down leaves the session as it was.
                warn!("couldn't renew the ChatGPT session: {err}");
                return refuse("the Codex sign-in couldn't be renewed, so try again");
            }
        };
        let Some(refresh_token) = tokens.refresh_token.clone() else {
            return refuse("OpenAI returned a renewal Codex can't use, so sign in again");
        };
        if !tokens.token_type.eq_ignore_ascii_case("bearer") {
            return refuse("OpenAI returned a renewal Codex can't use, so sign in again");
        }
        let Some(session) = saved.session.as_mut() else {
            return refuse("this device isn't signed in to Codex");
        };
        let now = self.now();
        session.access_token = tokens.access_token.clone();
        session.refresh_token = refresh_token;
        session.expires_at = now.after(Duration::from_secs(tokens.expires_in));
        session.earliest_refresh_at = earliest(tokens.earliest_refresh_at.as_ref());
        session.scopes = scopes(&tokens);
        let reply = TokenReply::Token {
            value: session.access_token.clone(),
            expires_at: session.expires_at,
        };
        let has_plan = session.has_plan();
        chatgpt.usable.store(has_plan, Ordering::SeqCst);
        // The old refresh token is spent, so the new one is kept in
        // memory even when the file can't take it.
        if let Err(err) = self.persist(&saved).await {
            warn!("couldn't store the renewed ChatGPT session: {err}");
        }
        info!("renewed the ChatGPT session");
        if has_plan {
            reply
        } else {
            refuse(NO_PLAN)
        }
    }

    /// Signs out: revokes the refresh token, then clears the session.
    /// Returns none when `credential` isn't the sign-in's.
    pub(super) async fn forget_chatgpt(&self, credential: CredentialId) -> Option<ActionOutcome> {
        if credential != credential_id(self.device()) {
            return None;
        }
        let chatgpt = self.chatgpt();
        let mut saved = chatgpt.saved.lock().await;
        let session = saved.session.take()?;
        chatgpt.usable.store(false, Ordering::SeqCst);
        let revoked = chatgpt
            .issuer
            .revoke(&session.client_id, &session.refresh_token)
            .await;
        if !revoked {
            warn!("OpenAI didn't confirm it revoked the ChatGPT session");
        }
        self.unpublish_session();
        if let Err(err) = self.persist(&saved).await {
            warn!("couldn't clear the ChatGPT session: {err}");
            return Some(refused(format!(
                "This device signed out but couldn't delete the session: {err}"
            )));
        }
        info!(revoked, "signed out of ChatGPT");
        Some(ActionOutcome::SignedOut { revoked })
    }

    /// Publishes the session's account and credential.
    fn publish_session(&self, session: &Session) {
        let device = self.device();
        let account = chatgpt_account(device);
        self.chatgpt()
            .usable
            .store(session.has_plan(), Ordering::SeqCst);
        let problem = (!session.has_plan()).then(|| NO_PLAN.to_owned());
        let credential = Credential {
            id: credential_id(device),
            device,
            kind: CredentialKind::ChatGpt {
                account,
                email: session.email.clone(),
            },
            stored_at: session.stored_at,
            problem,
        };
        let shown = login_account(
            account,
            device,
            Vendor::Codex,
            session.email.clone().unwrap_or_else(|| "Codex".to_owned()),
            LoginKind::ChatGptSignIn,
            codex_folder(&account.to_string()),
        );
        let shown = Account {
            email: session.email.clone(),
            ..shown
        };
        self.publish(credential, shown);
    }

    fn unpublish_session(&self) {
        let device = self.device();
        self.unpublish(credential_id(device), chatgpt_account(device));
    }
}
