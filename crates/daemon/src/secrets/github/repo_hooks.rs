// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! This device's webhook on each repository its GitHub tasks watch, made,
//! kept and deleted with a token that may manage that repository's
//! webhooks and nothing else. Each one's signing secret sits in a file of
//! its own and is read only to check a delivery's signature, and the
//! token in their URLs sits in another. Hooks this device didn't make are
//! never changed.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use futures::future::BoxFuture;
use maxhq_protocol::Secret;
use reqwest::Method;
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{json, Value};
use tracing::warn;

use super::{lock, GithubError, GithubToken, RepoName};
use crate::github_events::repo_hooks::{KeptHook, RepoHooks};
use crate::secrets::Secrets;

/// The file for the signing secret of repository webhook `hook`.
fn file(hook: u64) -> String {
    format!("github-hook-{hook}.secret")
}

/// The file for the token in this device's webhooks' URLs.
const DOORBELL_TOKEN: &str = "github-doorbell.token";

/// The shortest secret or token kept. A shorter one, such as an empty
/// file, counts as missing, so a new one replaces it.
const SHORTEST: usize = 32;

/// A new signing secret or token: 32 random bytes, URL-safe.
fn new_secret() -> Result<Secret, GithubError> {
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| GithubError::NoSecret("no random source".into()))?;
    Ok(Secret::new(URL_SAFE_NO_PAD.encode(bytes)))
}

/// Whether `hook` sends `events` to `url` as JSON, on.
fn matches(hook: &Value, url: &str, events: &[&str]) -> bool {
    let mut has: Vec<&str> = hook["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    has.sort_unstable();
    let mut wanted = events.to_vec();
    wanted.sort_unstable();
    hook["active"] == json!(true)
        && hook["config"]["url"] == json!(url)
        && hook["config"]["content_type"] == json!("json")
        && has == wanted
}

impl Secrets {
    /// `method` on the webhooks of `repo` at `path` below them, with a
    /// token that may manage them alone. GitHub answers 404 for what a
    /// token doesn't reach, which one kept from before may no longer, so
    /// such a 404 is asked again with a new token before it counts.
    async fn hooks_call(
        &self,
        repo: &RepoName,
        method: Method,
        path: &str,
        body: &Value,
    ) -> Result<Value, GithubError> {
        let path = format!("/repos/{}/{}/hooks{path}", repo.owner(), repo.name());
        let api = &self.inner.github.api;
        let (token, kept) = self.hooks_token(repo).await?;
        match api.send(method.clone(), &path, &token.value, body).await {
            Err(GithubError::Refused { status: 404, .. }) if kept => {
                self.drop_token(repo, true, &token);
                let (token, _) = self.hooks_token(repo).await?;
                api.send(method, &path, &token.value, body).await
            }
            sent => sent,
        }
    }

    /// A token that may manage `repo`'s webhooks alone, and whether it was
    /// kept from before.
    async fn hooks_token(&self, repo: &RepoName) -> Result<(GithubToken, bool), GithubError> {
        self.kept_token(repo, true).await.map_err(|err| match err {
            // GitHub won't mint a token for webhooks the installation
            // hasn't granted.
            GithubError::Refused {
                status: 422,
                message,
            } => GithubError::HooksNotGranted(message),
            err => err,
        })
    }

    /// This device's webhook on `repo`: `known` if GitHub still has it,
    /// else one already at `url`, whose token only this device knows, else
    /// a new one. It's set to send `events` to `url` as JSON, signed with
    /// the secret this device keeps. One that needs a new secret is left
    /// off until the secret and its ID are kept.
    async fn keep_github_hook(
        &self,
        repo: &RepoName,
        known: Option<u64>,
        url: &str,
        events: &[&str],
    ) -> Result<KeptHook, GithubError> {
        let listed = self
            .hooks_call(repo, Method::GET, "?per_page=100", &Value::Null)
            .await?;
        let hooks = listed.as_array().ok_or(GithubError::BadReply)?;
        let found = hooks
            .iter()
            .find(|hook| known.is_some() && hook["id"].as_u64() == known)
            .or_else(|| {
                hooks
                    .iter()
                    .find(|hook| hook["config"]["url"] == json!(url))
            });
        let id = found.and_then(|hook| hook["id"].as_u64());
        let stored = id.and_then(|id| self.github_hook_secret(id));
        let fresh = stored.is_none();
        let secret = match stored {
            Some(secret) => secret,
            None => new_secret()?,
        };
        let config = json!({
            "url": url,
            "content_type": "json",
            "insecure_ssl": "0",
            "secret": secret.expose(),
        });
        let body = json!({ "active": !fresh, "events": events, "config": config });
        let (id, changed) = match (found, id) {
            (Some(hook), Some(id)) if !fresh && matches(hook, url, events) => (id, false),
            (Some(_), Some(id)) => {
                self.hooks_call(repo, Method::PATCH, &format!("/{id}"), &body)
                    .await?;
                (id, true)
            }
            _ => {
                let mut body = body;
                body["name"] = json!("web");
                let made = self.hooks_call(repo, Method::POST, "", &body).await?;
                (made["id"].as_u64().ok_or(GithubError::BadReply)?, true)
            }
        };
        if fresh {
            self.inner
                .folder
                .write(&file(id), secret.expose().as_bytes())
                .map_err(|err| GithubError::NoSecret(err.to_string()))?;
        }
        Ok(KeptHook {
            id,
            changed,
            active: !fresh,
        })
    }

    /// Deletes this device's webhook `hook` on `repo`, and its secret. One
    /// already gone counts. One on a repository the App no longer reaches
    /// may still be there, so its secret stays.
    async fn remove_github_hook(&self, repo: &RepoName, hook: u64) -> Result<(), GithubError> {
        let deleted = self
            .hooks_call(repo, Method::DELETE, &format!("/{hook}"), &Value::Null)
            .await;
        match deleted {
            Ok(_) | Err(GithubError::Refused { status: 404, .. }) => {}
            Err(err) => return Err(err),
        }
        if let Err(err) = self.inner.folder.remove(&file(hook)) {
            warn!(hook, "couldn't delete a GitHub webhook's secret: {err}");
        }
        Ok(())
    }

    /// The signing secret of this device's webhook `hook`. One too short
    /// counts as missing, so the next keep sets a new one.
    fn github_hook_secret(&self, hook: u64) -> Option<Secret> {
        let read = self.inner.folder.read(&file(hook)).unwrap_or_else(|err| {
            warn!(hook, "couldn't read a GitHub webhook's secret: {err}");
            None
        });
        read.filter(|secret| secret.expose().len() >= SHORTEST)
    }

    /// The token in this device's webhooks' URLs, made the first time or
    /// when the one kept is too short.
    fn github_doorbell_token(&self) -> Option<Secret> {
        let folder = &self.inner.folder;
        match folder.read(DOORBELL_TOKEN) {
            Ok(Some(token)) if token.expose().len() >= SHORTEST => return Some(token),
            Ok(_) => {}
            Err(err) => {
                warn!("couldn't read the token GitHub's webhooks ring with: {err}");
                return None;
            }
        }
        let made = new_secret().and_then(|token| {
            folder
                .write(DOORBELL_TOKEN, token.expose().as_bytes())
                .map(|()| token)
                .map_err(|err| GithubError::NoSecret(err.to_string()))
        });
        made.map_err(|err| warn!("couldn't keep a token for GitHub's webhooks: {err}"))
            .ok()
    }

    /// The App's permissions page on GitHub, once a check named the App.
    fn github_permissions_page(&self) -> Option<String> {
        let app = self.inner.github.current()?;
        let found = lock(&app.found);
        let slug = found.slug.as_deref()?;
        Some(match &found.org {
            Some(org) => {
                format!("https://github.com/organizations/{org}/settings/apps/{slug}/permissions")
            }
            None => format!("https://github.com/settings/apps/{slug}/permissions"),
        })
    }
}

impl RepoHooks for Secrets {
    fn keep<'a>(
        &'a self,
        repo: &'a RepoName,
        known: Option<u64>,
        url: &'a str,
        events: &'a [&'a str],
    ) -> BoxFuture<'a, Result<KeptHook, GithubError>> {
        Box::pin(self.keep_github_hook(repo, known, url, events))
    }

    fn activate<'a>(
        &'a self,
        repo: &'a RepoName,
        hook: u64,
    ) -> BoxFuture<'a, Result<(), GithubError>> {
        Box::pin(async move {
            let body = json!({ "active": true });
            self.hooks_call(repo, Method::PATCH, &format!("/{hook}"), &body)
                .await
                .map(drop)
        })
    }

    fn ping<'a>(&'a self, repo: &'a RepoName, hook: u64) -> BoxFuture<'a, Result<(), GithubError>> {
        Box::pin(async move {
            let path = format!("/{hook}/pings");
            self.hooks_call(repo, Method::POST, &path, &Value::Null)
                .await
                .map(drop)
        })
    }

    fn remove<'a>(
        &'a self,
        repo: &'a RepoName,
        hook: u64,
    ) -> BoxFuture<'a, Result<(), GithubError>> {
        Box::pin(self.remove_github_hook(repo, hook))
    }

    fn secret(&self, hook: u64) -> Option<Secret> {
        self.github_hook_secret(hook)
    }

    fn permissions_page(&self) -> Option<String> {
        self.github_permissions_page()
    }

    fn doorbell_token(&self) -> Option<Secret> {
        self.github_doorbell_token()
    }
}
