// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! GitHub events: polling what happens on a repository for the tasks that
//! watch it, and handing each new event to the task as a request. Where
//! the device has a public address, a webhook on each watched repository
//! rings a doorbell for an event as it happens, and the polls slow down to
//! a backstop.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_protocol::ids::DeviceId;
use maxhq_protocol::time::Timestamp;
use serde_json::Value;
use tokio::sync::watch;

use crate::db::Db;
use crate::hooks::Webhooks;
use crate::secrets::{self, Fetched, GithubError, RepoName};
use crate::session::clock::{Clock, SystemClock};
use crate::session::Sessions;
use crate::tasks::Tasks;

mod cursors;
pub mod doorbell;
pub mod poll;
pub mod pollers;
pub mod repo_hooks;
mod seen;

/// How each watched repository's webhook stands, by [`RepoName::key`]. A
/// repository has an entry while this device wants a webhook on it.
pub type HookStates = BTreeMap<String, HookState>;

/// How a repository's webhook stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HookState {
    /// When it last delivered a request whose signature checked out.
    pub delivered: Option<Timestamp>,
    /// Whether it delivered in the last hour, so polling only backs it up.
    pub instant: bool,
    /// Why it couldn't be set up, until it can.
    pub problem: Option<HookProblem>,
}

/// Why a repository's webhook couldn't be set up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookProblem {
    pub reason: String,
    /// Where to fix it, such as the App's permissions page.
    pub link: Option<String>,
}

#[cfg(test)]
mod tests;

/// What GitHub events run on.
pub struct Wiring<'a> {
    pub sessions: &'a Sessions,
    /// The webhook listener the doorbell rings on, and what takes events.
    pub webhooks: Webhooks,
    /// This device's webhooks on GitHub.
    pub github: Arc<dyn repo_hooks::RepoHooks>,
    pub db: Db,
    pub device: DeviceId,
}

/// Starts the pollers, the webhooks' keeper and their doorbell, and shows
/// how they stand with the tasks. Without a doorbell token, which only
/// fails when the secrets folder does, GitHub tasks only poll.
pub fn install(wiring: Wiring<'_>) {
    let Wiring {
        sessions,
        webhooks,
        github,
        db,
        device,
    } = wiring;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let states = watch::Sender::new(HookStates::new());
    let pollers = pollers::start(
        sessions,
        Arc::new(AppEvents),
        Arc::new(webhooks.clone()),
        db.clone(),
        clock.clone(),
        states.clone(),
    );
    if let Some(token) = github.doorbell_token() {
        let path = repo_hooks::path(device);
        webhooks.set_doorbell(Arc::new(doorbell::GithubDoorbell::new(
            path.clone(),
            &token,
            pollers.clone(),
            github.clone(),
            db.clone(),
            clock.clone(),
            sessions.watch(),
            states.clone(),
        )));
        let keeper = repo_hooks::Keeper {
            path,
            token,
            tasks: sessions.watch(),
            door: Arc::new(webhooks),
            github,
            db,
            states: states.clone(),
            clock,
        };
        tokio::spawn(keeper.run());
    }
    sessions.follow_problems(pollers.problems());
    sessions.follow_webhooks(states.subscribe());
}

/// One read of a listing.
pub struct Page {
    /// Its items, or none new since the ETag.
    pub items: Fetched<Vec<Value>>,
    /// How long GitHub asks to wait before the next read.
    pub poll_after: Option<Duration>,
}

/// Where a repository's activity comes from.
pub trait EventSource: Send + Sync {
    /// The listing at `path` in the repository, such as its pull
    /// requests, or nothing new since `etag`.
    fn list<'a>(
        &'a self,
        repo: &'a RepoName,
        path: &'a str,
        etag: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Page, GithubError>>;

    /// What GitHub's API gives at `path` in the repository, such as one
    /// pull request, read fresh with the same token as its listings.
    fn read<'a>(
        &'a self,
        repo: &'a RepoName,
        path: &'a str,
    ) -> BoxFuture<'a, Result<Value, GithubError>>;
}

/// Listings from GitHub's API, read with this device's GitHub App.
pub struct AppEvents;

impl EventSource for AppEvents {
    fn list<'a>(
        &'a self,
        repo: &'a RepoName,
        path: &'a str,
        etag: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Page, GithubError>> {
        Box::pin(async move {
            let (items, poll_after) = secrets::github_poll(repo, path, etag).await?;
            Ok(Page { items, poll_after })
        })
    }

    fn read<'a>(
        &'a self,
        repo: &'a RepoName,
        path: &'a str,
    ) -> BoxFuture<'a, Result<Value, GithubError>> {
        Box::pin(async move {
            match secrets::github_poll(repo, path, None).await?.0 {
                Fetched::Fresh { value, .. } => Ok(value),
                Fetched::Unchanged => Err(GithubError::BadReply),
            }
        })
    }
}
