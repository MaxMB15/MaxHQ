// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The doorbell GitHub's repository webhooks ring, at this device's path
//! with a token only its webhooks' URLs hold. A request without a
//! signature or from a webhook this device didn't make is refused before
//! its body is read. A delivery only says something happened: once its
//! signature, event, delivery ID and rate check out, maxd reads the
//! event's action, number, repository, time and a push's ref and commits
//! from it, drops the rest, and builds the same event a poll would, which
//! the poller hands on as one it read. An event it can't build that way
//! makes the next poll come soon. One that comes while its repository has
//! no poller yet is turned away, so it can come again. Only the body's
//! SHA-256 is kept, so a replay of it does nothing, whatever delivery ID
//! it comes with. Nothing else from the body is logged, kept or handed on.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use futures::future::BoxFuture;
use hyper::StatusCode;
use maxhq_core::github_events::{hook_events, rung_event, Rung};
use maxhq_core::hooks::dedupe::REPLAY_MS;
use maxhq_core::hooks::verify::verify;
use maxhq_protocol::tasks::{DigestEncoding, SignatureCheck, Task, Trigger};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use tokio::sync::watch;
use tracing::warn;
use uuid::Uuid;

use super::pollers::Pollers;
use super::repo_hooks::RepoHooks;
use super::HookStates;
use crate::agents::{hash, same_hash};
use crate::db::Db;
use crate::hooks::{Counter, Doorbell, Slot};
use crate::secrets::RepoName;
use crate::session::clock::Clock;

/// How many deliveries a repository's webhook may ring with a minute.
pub const RATE: u32 = 120;
const RATE_SPAN_MS: u64 = 60_000;

/// The header GitHub signs each delivery's body in.
const SIGNATURE: &str = "x-hub-signature-256";
/// The header naming the webhook that sent a delivery.
const HOOK_ID: &str = "x-github-hook-id";

/// The doorbell.
pub struct GithubDoorbell {
    /// The device's path, which its token follows.
    path: String,
    /// The token's hash.
    token: Vec<u8>,
    pollers: Pollers,
    github: Arc<dyn RepoHooks>,
    db: Db,
    clock: Arc<dyn Clock>,
    tasks: watch::Receiver<Arc<[Task]>>,
    /// Where a verified delivery is noted, which slows the polls.
    states: watch::Sender<HookStates>,
    /// What each repository's webhook rang with lately, by key.
    rate: Mutex<Counter<String>>,
}

impl Doorbell for GithubDoorbell {
    fn rings(&self, path: &str) -> bool {
        let Some((at, token)) = path.rsplit_once('/') else {
            return false;
        };
        let door = at == self.path || self.path.strip_prefix("/maxhq") == Some(at);
        door && !token.is_empty() && same_hash(&hash(token), &self.token)
    }

    fn admit(&self, headers: &BTreeMap<String, String>) -> Result<(), (StatusCode, &'static str)> {
        let signed = headers
            .get(SIGNATURE)
            .is_some_and(|signature| !signature.trim().is_empty());
        let known = headers
            .get(HOOK_ID)
            .and_then(|hook| hook.trim().parse().ok())
            .and_then(|hook| self.db.github_hook_repo(hook).ok().flatten())
            .is_some();
        if signed && known {
            Ok(())
        } else {
            Err((StatusCode::UNAUTHORIZED, "invalid_signature"))
        }
    }

    fn ring(
        &self,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    ) -> BoxFuture<'_, (StatusCode, &'static str)> {
        Box::pin(async move { self.answer(&headers, body) })
    }
}

impl GithubDoorbell {
    /// A doorbell at `path` and `token` for the webhooks `github` keeps,
    /// handing what rings to `pollers` and noting each verified delivery
    /// in `states`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        path: String,
        token: &Secret,
        pollers: Pollers,
        github: Arc<dyn RepoHooks>,
        db: Db,
        clock: Arc<dyn Clock>,
        tasks: watch::Receiver<Arc<[Task]>>,
        states: watch::Sender<HookStates>,
    ) -> Self {
        Self {
            path,
            token: hash(token.expose()),
            pollers,
            github,
            db,
            clock,
            tasks,
            states,
            rate: Mutex::default(),
        }
    }

    /// Checks one delivery and starts what it asks for. A delivery is only
    /// noted once it's past the rate limit and read, so GitHub can send
    /// one dropped before then again.
    fn answer(
        &self,
        headers: &BTreeMap<String, String>,
        body: Vec<u8>,
    ) -> (StatusCode, &'static str) {
        let header = |name: &str| headers.get(name).map(String::as_str);
        let Some((key, repo)) = self.signed(headers, &body) else {
            return (StatusCode::UNAUTHORIZED, "invalid_signature");
        };
        let event = header("x-github-event").unwrap_or_default();
        if event != "ping" && !self.watched(&repo).contains(&event) {
            return (StatusCode::BAD_REQUEST, "event_not_watched");
        }
        // GitHub doesn't sign the delivery ID, so it's only trusted as
        // far as its shape.
        let Some(delivery) = header("x-github-delivery")
            .and_then(|delivery| Uuid::try_parse(delivery).ok())
            .map(|delivery| delivery.hyphenated().to_string())
        else {
            return (StatusCode::BAD_REQUEST, "missing_delivery");
        };
        let now = self.clock.wall();
        let slot = self
            .rate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take(key.clone(), now, RATE, RATE_SPAN_MS);
        match slot {
            Slot::Allowed => {}
            Slot::FirstRejected => {
                warn!(
                    repo = key,
                    "a GitHub webhook rang too often, so some deliveries were dropped"
                );
                return (StatusCode::TOO_MANY_REQUESTS, "rate_limited");
            }
            Slot::Rejected => return (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
        }
        if event == "ping" {
            // A ping carries nothing to run, and GitHub may send the same
            // one twice, so it isn't noted.
            self.delivered(&key, now);
            return (StatusCode::OK, "ping");
        }
        let Ok(rung) = serde_json::from_slice::<Rung>(&body) else {
            return (StatusCode::BAD_REQUEST, "bad_body");
        };
        let digest = ring::digest::digest(&ring::digest::SHA256, &body);
        drop(body);
        let named = rung
            .repository
            .as_ref()
            .and_then(|repository| RepoName::parse(&repository.full_name));
        if named.as_ref() != Some(&repo) {
            return (StatusCode::BAD_REQUEST, "wrong_repository");
        }
        if event == "push" && rung.deleted_ref() {
            return (StatusCode::ACCEPTED, "branch_deleted");
        }
        let happened = rung.happened(event);
        if happened.is_some_and(|at| at.0.saturating_add(REPLAY_MS) <= now.0) {
            return (StatusCode::ACCEPTED, "stale");
        }
        // With no poller to hand it to, as while maxd starts or a task is
        // turned on, a delivery would be noted and lost, so it isn't let
        // in, and GitHub's retry or Redeliver brings it again.
        if !self.pollers.polls(&repo) {
            return (StatusCode::SERVICE_UNAVAILABLE, "not_ready");
        }
        match self.db.first_github_body(digest.as_ref(), now) {
            Ok(true) => {}
            Ok(false) => return (StatusCode::ACCEPTED, "duplicate"),
            Err(err) => {
                warn!(repo = key, "couldn't note a GitHub delivery: {err}");
                return (StatusCode::SERVICE_UNAVAILABLE, "unavailable");
            }
        }
        self.delivered(&key, now);
        // An event without its time isn't one GitHub sent whole, so only a
        // poll reads it.
        let built = happened.and_then(|at| rung_event(&delivery, event, &rung, &rfc3339(at)));
        match built {
            Some(built) => {
                let pollers = self.pollers.clone();
                tokio::spawn(async move { pollers.ring(&repo, &built).await });
                (StatusCode::ACCEPTED, "accepted")
            }
            None => {
                self.pollers.nudge(&repo);
                (StatusCode::ACCEPTED, "nudged")
            }
        }
    }

    /// The repository whose webhook signed `body`, by key and name, when
    /// the hook is this device's and the signature is its secret's.
    fn signed(
        &self,
        headers: &BTreeMap<String, String>,
        body: &[u8],
    ) -> Option<(String, RepoName)> {
        let hook: u64 = headers.get(HOOK_ID)?.trim().parse().ok()?;
        let key = self.db.github_hook_repo(hook).ok()??;
        let secret = self.github.secret(hook)?;
        let check = SignatureCheck {
            header: SIGNATURE.into(),
            encoding: DigestEncoding::Hex,
            prefix: "sha256=".into(),
            has_secret: true,
        };
        if !verify(&check, secret.expose().as_bytes(), headers, body) {
            return None;
        }
        let repo = RepoName::parse(&key)?;
        Some((key, repo))
    }

    /// The webhook events the enabled tasks on `repo` want.
    fn watched(&self, repo: &RepoName) -> Vec<&'static str> {
        let tasks = self.tasks.borrow();
        let listed = tasks
            .iter()
            .filter(|task| task.enabled)
            .filter_map(|task| match &task.trigger {
                Trigger::GithubEvents {
                    repo: watched,
                    events,
                    ..
                } if RepoName::parse(watched).as_ref() == Some(repo) => Some(events),
                _ => None,
            })
            .flatten()
            .map(String::as_str);
        hook_events(listed)
    }

    /// Notes a verified delivery on the repository `key` names.
    fn delivered(&self, key: &str, now: Timestamp) {
        self.states
            .send_if_modified(|states| match states.get_mut(key) {
                Some(state) => {
                    state.delivered = Some(now);
                    state.instant = true;
                    true
                }
                None => false,
            });
    }
}

/// `at` as GitHub writes times, such as `2026-01-02T03:04:05Z`.
fn rfc3339(at: Timestamp) -> String {
    let millis = i64::try_from(at.0).unwrap_or(i64::MAX);
    jiff::Timestamp::from_millisecond(millis)
        .map(|at| at.strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}
