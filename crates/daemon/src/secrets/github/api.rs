// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The few calls to GitHub's REST API the App needs, and to the sign-in
//! endpoints your own sign-in needs, on the store's [`http`] client.

use std::collections::HashMap;
use std::time::Duration;

use maxhq_protocol::Secret;
use reqwest::header::{ACCEPT, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::{GithubError, RepoName};
use crate::secrets::http::{self, describe, form_post};

/// GitHub's API.
pub const GITHUB_API: &str = "https://api.github.com";
/// Where GitHub signs you in.
pub const GITHUB_LOGIN: &str = "https://github.com";
/// The REST API version the calls are written for.
pub(super) const API_VERSION: &str = "2022-11-28";
/// Items per page in a list.
const PAGE: usize = 100;
/// The most pages a list reads, so a server that never stops can't hold
/// the check forever.
const MAX_PAGES: usize = 50;
/// The header with how many seconds a poller should wait.
const POLL_INTERVAL: &str = "x-poll-interval";
/// The headers with the rate limit: requests left, allowed, and when the
/// hour starts over in seconds since 1970.
const RATE_REMAINING: &str = "x-ratelimit-remaining";
const RATE_LIMIT: &str = "x-ratelimit-limit";
const RATE_RESET: &str = "x-ratelimit-reset";

/// GitHub's API at `base`, and its sign-in endpoints at `login`.
pub struct Api {
    /// The client, or why it couldn't be built.
    http: Result<Client, String>,
    base: String,
    login: String,
}

/// What `GET /app` says.
#[derive(Deserialize)]
pub struct AppInfo {
    /// The App's URL name.
    pub slug: Option<String>,
    /// The App's public client ID, which its device flow signs you in
    /// through.
    #[serde(default)]
    pub client_id: Option<String>,
    /// What the App asks for where it's installed.
    #[serde(default)]
    pub permissions: Permissions,
    /// The account that owns the App.
    #[serde(default)]
    pub owner: Option<Owner>,
}

/// The account that owns the App.
#[derive(Deserialize)]
pub struct Owner {
    /// Its login.
    pub login: String,
    /// "User" or "Organization".
    #[serde(rename = "type", default)]
    pub kind: String,
}

/// One account the App is installed on.
#[derive(Deserialize)]
pub struct Installation {
    /// GitHub's ID for the installation.
    pub id: u64,
    /// The account it's on.
    #[serde(default)]
    pub account: Option<Account>,
    /// Set when the account's owner suspended the App.
    #[serde(default)]
    pub suspended_at: Option<String>,
    /// What the account granted. An account accepts permissions the App
    /// adds later on its own, so this can lag behind the App's.
    #[serde(default)]
    pub permissions: Permissions,
}

/// The account an installation is on.
#[derive(Deserialize)]
pub struct Account {
    /// Its login.
    pub login: String,
}

/// Permissions by name, as GitHub lists them.
#[derive(Default, Deserialize)]
pub struct Permissions(HashMap<String, Value>);

impl Permissions {
    /// What they allow on issues.
    pub fn issues(&self) -> Option<Access> {
        self.access("issues")
    }

    /// What they allow on `name`.
    fn access(&self, name: &str) -> Option<Access> {
        match self.0.get(name)?.as_str()? {
            "read" => Some(Access::Read),
            "write" => Some(Access::Write),
            _ => None,
        }
    }

    /// Which of [`READS`] they allow.
    pub fn reads(&self) -> Reads {
        let granted = |name| self.access(name).is_some();
        Reads {
            checks: granted("checks"),
            statuses: granted("statuses"),
            actions: granted("actions"),
        }
    }
}

/// The read permissions a PR's checks need, which an account has to accept
/// once the App adds them.
pub const READS: [&str; 3] = ["checks", "statuses", "actions"];

/// Which of [`READS`] an installation grants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reads {
    /// Check runs.
    pub checks: bool,
    /// Commit statuses.
    pub statuses: bool,
    /// Workflow runs.
    pub actions: bool,
}

impl Reads {
    fn granted(self) -> impl Iterator<Item = &'static str> {
        let each = [self.checks, self.statuses, self.actions];
        READS
            .into_iter()
            .zip(each)
            .filter_map(|(name, granted)| granted.then_some(name))
    }
}

/// How far a permission reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Reading only.
    Read,
    /// Reading and changing.
    Write,
}

impl Access {
    fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

/// A minted installation token.
#[derive(Deserialize)]
pub struct Minted {
    /// The token.
    pub token: Secret,
    /// When it stops working, in ISO 8601.
    pub expires_at: String,
    /// The repositories it reaches, when it was minted for some.
    #[serde(default)]
    pub repositories: Option<Vec<Repo>>,
}

/// A repository in a reply.
#[derive(Deserialize)]
pub struct Repo {
    /// "owner/name".
    pub full_name: String,
}

#[derive(Deserialize)]
struct Repos {
    total_count: usize,
    repositories: Vec<Repo>,
}

/// What GitHub's sign-in endpoints answer. Both answer a refusal with
/// 200 and say what it is in `error`.
#[derive(Default, Deserialize)]
pub struct LoginReply {
    /// The code a device flow polls with.
    pub device_code: Option<Secret>,
    /// The code you enter on GitHub.
    pub user_code: Option<String>,
    /// The page you enter it on.
    pub verification_uri: Option<String>,
    /// Seconds the code or the access token lasts. None for a token that
    /// doesn't expire.
    pub expires_in: Option<u64>,
    /// Seconds to wait between polls.
    pub interval: Option<u64>,
    /// Your user token.
    pub access_token: Option<Secret>,
    /// What renews it, when it expires.
    pub refresh_token: Option<Secret>,
    /// Seconds the refresh token lasts.
    pub refresh_token_expires_in: Option<u64>,
    /// Why GitHub refused, such as `authorization_pending`.
    pub error: Option<String>,
    /// GitHub's words for it.
    pub error_description: Option<String>,
}

/// Who a user token signs in as.
#[derive(Deserialize)]
pub struct User {
    /// Your login.
    pub login: String,
    /// GitHub's ID for you.
    pub id: u64,
}

/// What a GET that names an ETag got back.
pub enum Fetched<T> {
    /// GitHub sent it, with the ETag to ask with next time.
    Fresh {
        /// What it sent.
        value: T,
        /// Its ETag, if it gave one.
        etag: Option<String>,
    },
    /// Nothing changed since the ETag asked with. GitHub doesn't count
    /// this against the rate limit.
    Unchanged,
}

/// What a reply said of how fast to read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Paced {
    /// How long GitHub's `X-Poll-Interval` asks a poller to wait.
    pub poll_after: Option<Duration>,
    /// Requests left this hour.
    pub remaining: Option<u32>,
    /// Requests allowed this hour.
    pub limit: Option<u32>,
    /// When the hour starts over, in seconds since 1970.
    pub reset: Option<u64>,
}

/// What a token is minted for.
pub enum Scope<'a> {
    /// Reading the names of every repository the installation reaches.
    Listing,
    /// W-6's permissions on the one repository named `name`, with issues
    /// at `issues` and the check reads in `reads` where the installation
    /// grants them.
    Repo {
        /// The repository's name, without its owner.
        name: &'a str,
        /// What tokens may do with its issues.
        issues: Option<Access>,
        /// Which check reads tokens get.
        reads: Reads,
    },
    /// Managing the webhooks of the one repository named `name`, and
    /// nothing else.
    Hooks {
        /// The repository's name, without its owner.
        name: &'a str,
    },
}

impl Api {
    /// GitHub's API at `base` and its sign-in at `login`, through the
    /// proxy the environment names.
    pub fn new(base: &str, login: &str) -> Self {
        Self::build(base, login, true)
    }

    /// A test server that is both, reached directly.
    #[cfg(test)]
    pub fn local(base: &str) -> Self {
        Self::build(base, base, false)
    }

    fn build(base: &str, login: &str, proxies: bool) -> Self {
        Self {
            http: http::client(proxies),
            base: base.trim_end_matches('/').to_owned(),
            login: login.trim_end_matches('/').to_owned(),
        }
    }

    /// Posts `form` to the sign-in endpoint at `path`. A refusal GitHub
    /// names in `error` comes back as a reply, and any other status but
    /// 2xx as [`GithubError::Refused`].
    pub async fn login(
        &self,
        path: &str,
        form: &[(&str, &str)],
    ) -> Result<LoginReply, GithubError> {
        let http = self
            .http
            .as_ref()
            .map_err(|reason| GithubError::Unreachable(reason.clone()))?;
        let body = Zeroizing::new(
            url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(form)
                .finish(),
        );
        let response = form_post(http, &format!("{}{path}", self.login), &body)
            .send()
            .await
            .map_err(|err| GithubError::Unreachable(describe(&err)))?;
        let status = response.status();
        let body = Zeroizing::new(
            response
                .bytes()
                .await
                .map_err(|err| GithubError::Unreachable(describe(&err)))?
                .to_vec(),
        );
        match serde_json::from_slice::<LoginReply>(&body) {
            Ok(reply) if status.is_success() || reply.error.is_some() => Ok(reply),
            _ if status.is_success() => Err(GithubError::BadReply),
            _ => Err(GithubError::Refused {
                status: status.as_u16(),
                message: message(&body, status.canonical_reason()),
            }),
        }
    }

    /// `GET /user`: who `token` signs in as.
    pub async fn user(&self, token: &Secret) -> Result<User, GithubError> {
        self.call(self.request(Method::GET, "/user", token)?).await
    }

    /// `GET /app`, as the App.
    pub async fn app(&self, jwt: &Secret) -> Result<AppInfo, GithubError> {
        self.call(self.request(Method::GET, "/app", jwt)?).await
    }

    /// Every installation of the App.
    pub async fn installations(&self, jwt: &Secret) -> Result<Vec<Installation>, GithubError> {
        let mut all = Vec::new();
        for page in 1..=MAX_PAGES {
            let path = format!("/app/installations?per_page={PAGE}&page={page}");
            let batch: Vec<Installation> =
                self.call(self.request(Method::GET, &path, jwt)?).await?;
            let last = batch.len() < PAGE;
            all.extend(batch);
            if last {
                break;
            }
        }
        Ok(all)
    }

    /// The installation that reaches `repo`, or none when the App isn't
    /// installed there.
    pub async fn repo_installation(
        &self,
        jwt: &Secret,
        repo: &RepoName,
    ) -> Result<Option<Installation>, GithubError> {
        let path = format!("/repos/{}/{}/installation", repo.owner(), repo.name());
        match self.call(self.request(Method::GET, &path, jwt)?).await {
            Ok(found) => Ok(Some(found)),
            Err(GithubError::Refused { status: 404, .. }) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Mints a token from `installation` for `scope`.
    pub async fn mint(
        &self,
        jwt: &Secret,
        installation: u64,
        scope: Scope<'_>,
    ) -> Result<Minted, GithubError> {
        let body = match scope {
            Scope::Listing => json!({ "permissions": { "metadata": "read" } }),
            Scope::Repo {
                name,
                issues,
                reads,
            } => {
                let mut permissions = json!({
                    "contents": "write",
                    "pull_requests": "write",
                    "metadata": "read",
                });
                if let Some(issues) = issues {
                    permissions["issues"] = json!(issues.as_str());
                }
                for read in reads.granted() {
                    permissions[read] = json!("read");
                }
                json!({ "repositories": [name], "permissions": permissions })
            }
            Scope::Hooks { name } => json!({
                "repositories": [name],
                "permissions": { "repository_hooks": "write", "metadata": "read" },
            }),
        };
        let path = format!("/app/installations/{installation}/access_tokens");
        let request = self
            .request(Method::POST, &path, jwt)?
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_string());
        self.call(request).await
    }

    /// The names of the repositories an installation token reaches.
    pub async fn installation_repos(&self, token: &Secret) -> Result<Vec<String>, GithubError> {
        let mut names = Vec::new();
        for page in 1..=MAX_PAGES {
            let path = format!("/installation/repositories?per_page={PAGE}&page={page}");
            let batch: Repos = self.call(self.request(Method::GET, &path, token)?).await?;
            let got = batch.repositories.len();
            names.extend(batch.repositories.into_iter().map(|repo| repo.full_name));
            if got < PAGE || names.len() >= batch.total_count {
                break;
            }
        }
        Ok(names)
    }

    /// `GET path` with an installation token, asking only for a change
    /// since `etag`, and how long GitHub's `X-Poll-Interval` asks to wait
    /// before the next poll, when it sends one.
    pub async fn get_polled<T: DeserializeOwned>(
        &self,
        token: &Secret,
        path: &str,
        etag: Option<&str>,
    ) -> Result<(Fetched<T>, Option<Duration>), GithubError> {
        let (fetched, paced) = self.get_paced(token, path, etag).await?;
        Ok((fetched, paced.poll_after))
    }

    /// [`Api::get_polled`], with all the reply said of how fast to read.
    pub async fn get_paced<T: DeserializeOwned>(
        &self,
        token: &Secret,
        path: &str,
        etag: Option<&str>,
    ) -> Result<(Fetched<T>, Paced), GithubError> {
        let mut request = self.request(Method::GET, path, token)?;
        if let Some(etag) = etag {
            request = request.header(IF_NONE_MATCH, etag);
        }
        let response = request
            .send()
            .await
            .map_err(|err| GithubError::Unreachable(describe(&err)))?;
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let number = |name| header(name).and_then(|value| value.trim().parse().ok());
        let paced = Paced {
            poll_after: number(POLL_INTERVAL).map(Duration::from_secs),
            remaining: number(RATE_REMAINING).and_then(|left| u32::try_from(left).ok()),
            limit: number(RATE_LIMIT).and_then(|limit| u32::try_from(limit).ok()),
            reset: number(RATE_RESET),
        };
        if response.status() == StatusCode::NOT_MODIFIED {
            return Ok((Fetched::Unchanged, paced));
        }
        let etag = header(ETAG.as_str());
        let value = read(response).await?;
        Ok((Fetched::Fresh { value, etag }, paced))
    }

    /// `method path` with a JSON `body`, none when it's null, as whoever
    /// `token` is, such as you with your user token. An empty reply reads
    /// as null.
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        token: &Secret,
        body: &Value,
    ) -> Result<Value, GithubError> {
        let mut request = self.request(method, path, token)?;
        if !body.is_null() {
            request = request
                .header(CONTENT_TYPE, "application/json")
                .body(body.to_string());
        }
        self.call(request).await
    }

    /// Asks GitHub to end installation `token` now rather than at its
    /// expiry.
    pub async fn revoke_installation_token(&self, token: &Secret) -> Result<(), GithubError> {
        self.send(Method::DELETE, "/installation/token", token, &Value::Null)
            .await
            .map(|_| ())
    }

    /// A request to `path`, carrying `auth` in a header reqwest marks
    /// sensitive.
    fn request(
        &self,
        method: Method,
        path: &str,
        auth: &Secret,
    ) -> Result<RequestBuilder, GithubError> {
        let http = self
            .http
            .as_ref()
            .map_err(|reason| GithubError::Unreachable(reason.clone()))?;
        Ok(http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(auth.expose())
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION))
    }

    /// Sends `request` and reads a JSON reply. Any status but 2xx comes
    /// back as [`GithubError::Refused`] with GitHub's message.
    async fn call<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T, GithubError> {
        let response = request
            .send()
            .await
            .map_err(|err| GithubError::Unreachable(describe(&err)))?;
        read(response).await
    }
}

/// Reads a JSON reply, an empty one as null. Any status but 2xx comes back
/// as [`GithubError::Refused`] with GitHub's message.
async fn read<T: DeserializeOwned>(response: Response) -> Result<T, GithubError> {
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|err| GithubError::Unreachable(describe(&err)))?;
    if !status.is_success() {
        return Err(GithubError::Refused {
            status: status.as_u16(),
            message: message(&body, status.canonical_reason()),
        });
    }
    let body: &[u8] = if body.is_empty() { b"null" } else { &body };
    serde_json::from_slice(body).map_err(|_| GithubError::BadReply)
}

/// GitHub's own words for a refusal, from its JSON body, kept short.
pub(super) fn message(body: &[u8], reason: Option<&str>) -> String {
    let said = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.get("message")?.as_str().map(str::to_owned));
    let text = said.unwrap_or_else(|| reason.unwrap_or("no reason given").to_owned());
    let line = text.lines().next().unwrap_or_default();
    line.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_githubs_message_short() {
        let body = br#"{"message":"Bad credentials\nmore","documentation_url":"x"}"#;
        assert_eq!(message(body, Some("Unauthorized")), "Bad credentials");
        assert_eq!(message(b"<html>", Some("Bad Gateway")), "Bad Gateway");
        let long = format!(r#"{{"message":"{}"}}"#, "a".repeat(500));
        assert_eq!(message(long.as_bytes(), None).len(), 300);
    }
}
