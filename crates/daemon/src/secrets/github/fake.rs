// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A GitHub that runs inside the test, on 127.0.0.1. It checks each JWT
//! against the test key, mints tokens such as
//! `ghs_test-not-a-real-token-1`, and keeps every request it got. A repo
//! token reads one repo's pull requests, issues and events, and checks
//! where the installation grants them, and a read comes with an ETag that
//! a repeat can name to get 304. A user token merges and approves as its
//! login. A repo token that may manage webhooks lists, makes, changes,
//! deletes and pings them, keeping each one's secret for the test to read.
//! Its `gh` stands in
//! for the real one, answering `gh api` from the same lists as `gh` prints
//! them. It also signs you in through the App's device flow and hands out
//! user tokens such as `ghu_test-not-a-real-token-1`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ring::signature::{UnparsedPublicKey, RSA_PKCS1_2048_8192_SHA256};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::gh::{Ran, RunError};
use super::key::AppKey;
use super::{Api, Gh};
use crate::secrets::testing::app_key;

/// The test App's ID.
pub const APP_ID: u64 = 4242;
/// The test App's URL name.
pub const SLUG: &str = "maxhq-test";
/// What every minted token starts with.
pub const TOKEN_PREFIX: &str = "ghs_test-not-a-real-token-";
/// The test App's client ID.
pub const CLIENT_ID: &str = "Iv23test0client";
/// What every user token starts with.
pub const USER_TOKEN_PREFIX: &str = "ghu_test-not-a-real-token-";
/// What every refresh token starts with.
pub const REFRESH_PREFIX: &str = "ghr_test-not-a-real-token-";
/// The device code every device flow polls with.
pub const DEVICE_CODE: &str = "test-not-a-real-device-code";
/// The code you'd enter on GitHub.
pub const USER_CODE: &str = "ABCD-1234";
/// Who every user token signs in as.
pub const LOGIN: &str = "octocat";

/// The server, and what it knows.
pub struct Fake {
    base: String,
    state: Arc<Mutex<State>>,
}

/// What the fake GitHub holds.
pub struct State {
    /// The App's public key, as PKCS #1.
    pub public_der: Vec<u8>,
    /// The App ID the JWTs must name.
    pub app_id: u64,
    /// What the App asks for on issues, such as "write".
    pub app_issues: Option<&'static str>,
    /// Where the App is installed.
    pub installations: Vec<Install>,
    /// The `expires_at` every token gets.
    pub expires_at: String,
    /// Whether it drops every connection unanswered, as if GitHub were out
    /// of reach.
    pub offline: bool,
    /// Every request, oldest first.
    pub requests: Vec<Request>,
    /// Every repo's pull requests, as `/pulls` gives them.
    pub pulls: Vec<Value>,
    /// Every repo's issues, as `/issues` gives them, pull requests among
    /// them as on GitHub.
    pub issues: Vec<Value>,
    /// Pull requests past the list's first page, which only a lookup by
    /// number finds.
    pub past_pulls: Vec<Value>,
    /// Issues past the list's first page, the same way.
    pub past_issues: Vec<Value>,
    /// Every repo's branches, as `/branches` gives them.
    pub branches: Vec<Value>,
    /// What any search finds.
    pub searched: Vec<Value>,
    /// Every repo's events, as `/events` gives them.
    pub events: Vec<Value>,
    /// The `X-Poll-Interval` every GET answers with, if any.
    pub poll_interval: Option<u64>,
    /// The rate limit every GET answers with, if any: requests left,
    /// allowed, and when the hour starts over in seconds since 1970.
    pub rate: Option<(u32, u32, u64)>,
    /// The merge settings every repo answers with, such as
    /// `allow_squash_merge`, where GitHub would give them.
    pub settings: Value,
    /// Check runs by commit, as `check-runs` gives them.
    pub checks: HashMap<String, Value>,
    /// Statuses by commit, as `status` gives them.
    pub statuses: HashMap<String, Value>,
    /// The workflow runs any commit has.
    pub runs: Vec<Value>,
    /// Reviews by PR number.
    pub reviews: HashMap<u64, Vec<Value>>,
    /// Conversation comments by PR number.
    pub comments: HashMap<u64, Vec<Value>>,
    /// Changed files by PR number.
    pub files: HashMap<u64, Vec<Value>>,
    /// User tokens and the login each acts as.
    pub users: HashMap<String, String>,
    /// Whether the users may write to the repos.
    pub users_write: bool,
    /// Whether a merge waits in a merge queue, leaving the PR open.
    pub queues_merges: bool,
    /// How many GETs came back as 304.
    pub unchanged: usize,
    /// Every repo's webhooks, each with its `repo` as "owner/name" and its
    /// config's secret in the clear.
    pub hooks: Vec<Value>,
    /// The webhooks pinged, by ID, oldest first.
    pub pings: Vec<u64>,
    /// The organization that owns the App, or none when a user does.
    pub app_org: Option<&'static str>,
    hook_ids: u64,
    /// What `gh` is on the device.
    pub gh: GhLogin,
    /// The arguments of every `gh` run, oldest first.
    pub gh_runs: Vec<Vec<String>>,
    minted: u64,
    /// Your side of GitHub.
    pub login: Login,
    /// The installation each listing token is for.
    listing: HashMap<String, u64>,
    /// The repo each repo token reaches, as lowercase "owner/name", and
    /// what it may read.
    repo_tokens: HashMap<String, (String, Grants)>,
}

/// What a token may read beyond pull requests.
#[derive(Clone, Copy)]
struct Grants {
    issues: bool,
    /// Checks, statuses and actions.
    reads: bool,
    /// Managing webhooks.
    hooks: bool,
}

/// The App's device flow and the user tokens it gives.
pub struct Login {
    /// Whether the App has Device Flow ticked.
    pub device_flow: bool,
    /// What each poll answers before the one that gives a token, oldest
    /// first, such as `authorization_pending`.
    pub polls: VecDeque<&'static str>,
    /// Seconds an access token lasts, or none for one that doesn't expire
    /// and comes with no refresh token.
    pub expires_in: Option<u64>,
    /// Seconds a refresh token lasts.
    pub refresh_expires_in: u64,
    /// Whether a refresh fails with 502, as when GitHub is down.
    pub down: bool,
    /// The access tokens `/user` takes.
    pub valid: HashSet<String>,
    /// The one refresh token that works, since each refresh rotates it.
    pub refresh: Option<String>,
    issued: u64,
}

/// What `gh` is on the test's device.
pub enum GhLogin {
    /// It isn't installed.
    Missing,
    /// It's installed with no sign-in.
    SignedOut,
    /// It's signed in to an account that sees these repos, "owner/name",
    /// issues and all.
    SignedIn(Vec<String>),
}

/// One installation of the App.
pub struct Install {
    /// GitHub's ID for it.
    pub id: u64,
    /// The account it's on.
    pub owner: String,
    /// The names of the repositories it reaches.
    pub repos: Vec<String>,
    /// Whether the account suspended the App.
    pub suspended: bool,
    /// What the account granted on issues.
    pub issues: Option<&'static str>,
    /// Whether the account granted reading checks, statuses and actions.
    pub reads: bool,
    /// Whether the account granted managing webhooks.
    pub hooks: bool,
}

impl Install {
    /// What the account granted the App.
    fn permissions(&self) -> Value {
        let mut permissions = json!({
            "contents": "write",
            "pull_requests": "write",
            "metadata": "read",
        });
        if let Some(issues) = self.issues {
            permissions["issues"] = json!(issues);
        }
        if self.reads {
            for read in ["checks", "statuses", "actions"] {
                permissions[read] = json!("read");
            }
        }
        if self.hooks {
            permissions["repository_hooks"] = json!("write");
        }
        permissions
    }
}

/// A request the server got.
#[derive(Clone, Debug)]
pub struct Request {
    /// Such as "POST".
    pub method: String,
    /// The path, without the query.
    pub path: String,
    /// The query, as sent.
    pub query: String,
    /// The body, parsed when it's JSON, and an object of its fields when
    /// it's a form.
    pub body: Value,
    /// The bearer token it carried.
    pub auth: Option<String>,
}

impl Fake {
    /// Starts a server that takes JWTs the test App's key signed, with the
    /// App installed nowhere yet.
    pub async fn start() -> Self {
        let public_der = AppKey::parse(&app_key()).unwrap().public_der();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State {
            public_der,
            app_id: APP_ID,
            app_issues: Some("write"),
            installations: Vec::new(),
            expires_at: "2026-10-25T13:00:00Z".to_owned(),
            requests: Vec::new(),
            pulls: Vec::new(),
            issues: Vec::new(),
            past_pulls: Vec::new(),
            branches: Vec::new(),
            past_issues: Vec::new(),
            searched: Vec::new(),
            events: Vec::new(),
            poll_interval: None,
            rate: None,
            settings: json!({}),
            checks: HashMap::new(),
            statuses: HashMap::new(),
            runs: Vec::new(),
            reviews: HashMap::new(),
            comments: HashMap::new(),
            files: HashMap::new(),
            users: HashMap::new(),
            users_write: true,
            queues_merges: false,
            unchanged: 0,
            hooks: Vec::new(),
            pings: Vec::new(),
            app_org: None,
            hook_ids: 0,
            gh: GhLogin::Missing,
            gh_runs: Vec::new(),
            minted: 0,
            login: Login {
                device_flow: true,
                polls: VecDeque::new(),
                expires_in: Some(8 * 60 * 60),
                refresh_expires_in: 184 * 24 * 60 * 60,
                down: false,
                valid: HashSet::new(),
                refresh: None,
                issued: 0,
            },
            listing: HashMap::new(),
            repo_tokens: HashMap::new(),
            offline: false,
        }));
        let serving = state.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let state = serving.clone();
                tokio::spawn(async move {
                    let _ = serve(stream, &state).await;
                });
            }
        });
        Self { base, state }
    }

    /// What the server knows, for the test to read or change.
    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// A client for it.
    pub fn api(&self) -> Api {
        Api::local(&self.base)
    }

    /// A `gh` that reads from it.
    pub fn gh(&self) -> Gh {
        let state = self.state.clone();
        Gh::fake(move |args| state.lock().unwrap().gh_api(args))
    }

    /// The body of every token request for chosen repositories.
    pub fn repo_mints(&self) -> Vec<(String, Value)> {
        self.state()
            .requests
            .iter()
            .filter(|request| {
                request.method == "POST" && request.body.get("repositories").is_some()
            })
            .map(|request| (request.path.clone(), request.body.clone()))
            .collect()
    }
}

/// Answers one request on `stream`, then closes it.
async fn serve(mut stream: TcpStream, state: &Mutex<State>) -> std::io::Result<()> {
    let mut read = Vec::new();
    let mut chunk = [0; 4096];
    let head_end = loop {
        let got = stream.read(&mut chunk).await?;
        if got == 0 {
            return Ok(());
        }
        read.extend_from_slice(&chunk[..got]);
        if let Some(at) = read.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&read[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split(' ');
    let method = first.next().unwrap_or_default().to_owned();
    let target = first.next().unwrap_or_default().to_owned();
    let mut length = 0;
    let mut auth = None;
    let mut if_none_match = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse().unwrap_or(0),
            "authorization" => auth = value.trim().strip_prefix("Bearer ").map(str::to_owned),
            "if-none-match" => if_none_match = Some(value.trim().to_owned()),
            _ => {}
        }
    }
    while read.len() < head_end + length {
        let got = stream.read(&mut chunk).await?;
        if got == 0 {
            break;
        }
        read.extend_from_slice(&chunk[..got]);
    }
    let body = &read[head_end..(head_end + length).min(read.len())];

    let (status, reply, poll, rate) = {
        let mut state = state.lock().unwrap();
        if state.offline {
            return Ok(());
        }
        let (status, reply) = state.answer(&method, &target, auth.as_deref(), body);
        (status, reply, state.poll_interval, state.rate)
    };
    let rate = rate
        .filter(|_| method == "GET")
        .map(|(left, limit, reset)| {
            format!(
                "x-ratelimit-remaining: {left}\r\nx-ratelimit-limit: {limit}\r\nx-ratelimit-reset: {reset}\r\n"
            )
        })
        .unwrap_or_default();
    let poll = poll
        .filter(|_| method == "GET")
        .map(|secs| format!("x-poll-interval: {secs}\r\n{rate}"))
        .unwrap_or(rate);
    // A 204 has no body.
    let reply = if status == 204 {
        String::new()
    } else {
        reply.to_string()
    };
    let etag = (method == "GET" && status == 200).then(|| etag_of(&reply));
    if etag.is_some() && etag == if_none_match {
        state.lock().unwrap().unchanged += 1;
        let response = format!(
            "HTTP/1.1 304 Not Modified\r\n{poll}content-length: 0\r\nconnection: close\r\n\r\n"
        );
        stream.write_all(response.as_bytes()).await?;
        return stream.shutdown().await;
    }
    let etag = etag
        .map(|etag| format!("etag: {etag}\r\n"))
        .unwrap_or_default();
    let response = format!(
        "HTTP/1.1 {status} Fake\r\ncontent-type: application/json\r\n{etag}{poll}content-length: {}\r\nconnection: close\r\n\r\n{reply}",
        reply.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

/// The ETag of a reply.
fn etag_of(reply: &str) -> String {
    let mut hasher = DefaultHasher::new();
    reply.hash(&mut hasher);
    format!("\"{:x}\"", hasher.finish())
}

impl State {
    /// Takes its repository from each repo token minted so far, as when
    /// the App's access to it changed. GitHub then answers 404 there for
    /// the token, as for any repository a token doesn't reach.
    pub fn narrow_repo_tokens(&mut self) {
        for (repo, _) in self.repo_tokens.values_mut() {
            repo.clear();
        }
    }

    /// Answers a `gh api` run the way `gh` does with `--include`: the
    /// status and headers, then the body, exiting 1 past 299 and 4 with no
    /// sign-in.
    fn gh_api(&mut self, args: &[String]) -> Result<Ran, RunError> {
        self.gh_runs.push(args.to_vec());
        let repos = match &self.gh {
            GhLogin::Missing => return Err(RunError::Missing),
            GhLogin::SignedOut => {
                return Ok(Ran {
                    code: Some(4),
                    stdout: Vec::new(),
                    stderr: b"To get started with GitHub CLI, please run:  gh auth login\n"
                        .to_vec(),
                })
            }
            GhLogin::SignedIn(repos) => repos.clone(),
        };
        let target = args.last().cloned().unwrap_or_default();
        let if_none_match = args
            .windows(2)
            .filter(|pair| pair[0] == "-H")
            .find_map(|pair| pair[1].strip_prefix("If-None-Match: "));
        let (path, query) = target.split_once('?').unwrap_or((&target, ""));
        self.requests.push(Request {
            method: "GET".to_owned(),
            path: path.to_owned(),
            query: query.to_owned(),
            body: Value::Null,
            auth: None,
        });
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let (status, reply) = self
            .repo_read(
                "GET",
                &parts,
                Some((
                    &repos,
                    Grants {
                        issues: true,
                        reads: true,
                        hooks: false,
                    },
                )),
            )
            .unwrap_or_else(|| (404, json!({ "message": "Not Found" })));
        let reply = reply.to_string();
        let etag = (status == 200).then(|| etag_of(&reply));
        if etag.is_some() && etag.as_deref() == if_none_match {
            self.unchanged += 1;
            return Ok(Ran {
                code: Some(1),
                stdout: b"HTTP/2.0 304 Not Modified\n\r\n".to_vec(),
                stderr: b"gh: HTTP 304\n".to_vec(),
            });
        }
        let etag = etag
            .map(|etag| format!("Etag: {etag}\r\n"))
            .unwrap_or_default();
        let stdout = format!(
            "HTTP/2.0 {status} Fake\nContent-Type: application/json; charset=utf-8\r\n{etag}\r\n{reply}"
        );
        let failed = status > 299;
        Ok(Ran {
            code: Some(i32::from(failed)),
            stdout: stdout.into_bytes(),
            stderr: if failed {
                format!("gh: HTTP {status}\n").into_bytes()
            } else {
                Vec::new()
            },
        })
    }

    fn answer(
        &mut self,
        method: &str,
        target: &str,
        auth: Option<&str>,
        body: &[u8],
    ) -> (u16, Value) {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let form: HashMap<String, String> =
            url::form_urlencoded::parse(body).into_owned().collect();
        self.requests.push(Request {
            method: method.to_owned(),
            path: path.to_owned(),
            query: query.to_owned(),
            body: serde_json::from_slice(body).unwrap_or_else(|_| json!(form)),
            auth: auth.map(str::to_owned),
        });
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();

        if let Some(reply) = self.sign_in(method, path, auth, &form) {
            return reply;
        }
        if let Some(login) = auth.and_then(|token| self.users.get(token)).cloned()
            && let Some(reply) = self.as_user(method, &parts, &login, body)
        {
            return reply;
        }
        let reached = auth.and_then(|token| self.repo_tokens.get(token)).cloned();
        let reach = reached
            .as_ref()
            .map(|(repo, grants)| (std::slice::from_ref(repo), *grants));
        if let Some(reply) = self.repo_read(method, &parts, reach) {
            return reply;
        }
        if let Some(reply) = self.repo_hooks(method, &parts, reach, body) {
            return reply;
        }
        if method == "DELETE" && path == "/installation/token" {
            let known = auth.is_some_and(|token| {
                self.listing.remove(token).is_some() || self.repo_tokens.remove(token).is_some()
            });
            return if known {
                (204, Value::Null)
            } else {
                (401, json!({ "message": "Bad credentials" }))
            };
        }
        if method == "GET" && path == "/installation/repositories" {
            let Some(id) = auth.and_then(|token| self.listing.get(token)).copied() else {
                return (401, json!({ "message": "Bad credentials" }));
            };
            return (200, self.repositories(id, query));
        }
        if !self.signed(auth) {
            return (
                401,
                json!({ "message": "A JSON web token could not be decoded" }),
            );
        }
        match (method, parts.as_slice()) {
            ("GET", ["app"]) => {
                let mut permissions = json!({
                    "contents": "write",
                    "pull_requests": "write",
                    "metadata": "read",
                });
                if let Some(issues) = self.app_issues {
                    permissions["issues"] = json!(issues);
                }
                let owner = match self.app_org {
                    Some(org) => json!({ "login": org, "type": "Organization" }),
                    None => json!({ "login": LOGIN, "type": "User" }),
                };
                (
                    200,
                    json!({ "id": self.app_id, "slug": SLUG, "client_id": CLIENT_ID, "name": "MaxHQ test", "owner": owner, "permissions": permissions }),
                )
            }
            ("GET", ["app", "installations"]) => {
                let page = query_number(query, "page").unwrap_or(1);
                let all: Vec<Value> = self
                    .installations
                    .iter()
                    .map(|install| {
                        json!({
                            "id": install.id,
                            "account": { "login": install.owner },
                            "suspended_at": install.suspended.then_some("2026-10-01T00:00:00Z"),
                            "permissions": install.permissions(),
                        })
                    })
                    .collect();
                (200, Value::Array(if page == 1 { all } else { Vec::new() }))
            }
            ("GET", ["repos", owner, name, "installation"]) => match self.reaching(owner, name) {
                Some(install) => (
                    200,
                    json!({
                        "id": install.id,
                        "account": { "login": install.owner },
                        "permissions": install.permissions(),
                    }),
                ),
                None => (404, json!({ "message": "Not Found" })),
            },
            ("POST", ["app", "installations", id, "access_tokens"]) => {
                let id: u64 = id.parse().unwrap_or(0);
                self.mint(id, body)
            }
            _ => (404, json!({ "message": "Not Found" })),
        }
    }

    /// Answers a read by something that reaches `reach`'s repos, "owner/name",
    /// and their issues when it says so: the pull request and issue lists,
    /// one of either by number, the branches, and the search. None for any
    /// other path.
    fn repo_read(
        &self,
        method: &str,
        parts: &[&str],
        reach: Option<(&[String], Grants)>,
    ) -> Option<(u16, Value)> {
        let denied = (401, json!({ "message": "Bad credentials" }));
        let forbidden = (
            403,
            json!({ "message": "Resource not accessible by integration" }),
        );
        let missing = (404, json!({ "message": "Not Found" }));
        let numbered = |list: &[Value], past: &[Value], number: &str| {
            list.iter()
                .chain(past)
                .find(|item| item["number"].as_u64() == number.parse().ok())
                .cloned()
        };
        if method != "GET" {
            return None;
        }
        Some(match parts {
            ["search", "issues"] => match reach {
                None => denied,
                Some(_) => (
                    200,
                    json!({ "total_count": self.searched.len(), "items": self.searched }),
                ),
            },
            ["repos", owner, name] => match reach {
                None => denied,
                Some((repos, _))
                    if repos
                        .iter()
                        .any(|repo| repo.eq_ignore_ascii_case(&format!("{owner}/{name}"))) =>
                {
                    let mut repo = json!({ "full_name": format!("{owner}/{name}") });
                    if let (Some(repo), Some(settings)) =
                        (repo.as_object_mut(), self.settings.as_object())
                    {
                        repo.extend(settings.clone());
                    }
                    (200, repo)
                }
                Some(_) => missing,
            },
            ["repos", owner, name, what, rest @ ..]
                if matches!(
                    *what,
                    "pulls" | "issues" | "events" | "commits" | "actions" | "branches"
                ) =>
            {
                let Some((repos, grants)) = reach else {
                    return Some(denied);
                };
                let asked = format!("{owner}/{name}");
                if !repos.iter().any(|repo| repo.eq_ignore_ascii_case(&asked)) {
                    return Some(missing);
                }
                let listed = |lists: &HashMap<u64, Vec<Value>>, number: &str| {
                    let number = number.parse().unwrap_or(0);
                    (
                        200,
                        Value::Array(lists.get(&number).cloned().unwrap_or_default()),
                    )
                };
                match (*what, rest) {
                    // A PR's conversation reads with pull requests alone.
                    ("issues", [number, "comments"]) => listed(&self.comments, number),
                    ("issues", _) if !grants.issues => forbidden,
                    ("commits" | "actions", _) if !grants.reads => forbidden,
                    ("commits", [sha, "check-runs"]) => (
                        200,
                        self.checks
                            .get(*sha)
                            .cloned()
                            .unwrap_or_else(|| json!({ "total_count": 0, "check_runs": [] })),
                    ),
                    ("commits", [sha, "status"]) => (
                        200,
                        self.statuses
                            .get(*sha)
                            .cloned()
                            .unwrap_or_else(|| json!({ "state": "pending", "statuses": [] })),
                    ),
                    ("actions", ["runs"]) => (
                        200,
                        json!({ "total_count": self.runs.len(), "workflow_runs": self.runs }),
                    ),
                    ("pulls", [number, "reviews"]) => listed(&self.reviews, number),
                    ("pulls", [number, "files"]) => listed(&self.files, number),
                    ("pulls", []) => (200, Value::Array(self.pulls.clone())),
                    ("issues", []) => (200, Value::Array(self.issues.clone())),
                    ("events", []) => (200, Value::Array(self.events.clone())),
                    ("branches", []) => (200, Value::Array(self.branches.clone())),
                    ("pulls", [number]) => numbered(&self.pulls, &self.past_pulls, number)
                        .map_or(missing, |pull| (200, pull)),
                    ("issues", [number]) => numbered(&self.issues, &self.past_issues, number)
                        .map_or(missing, |issue| (200, issue)),
                    _ => missing,
                }
            }
            _ => return None,
        })
    }

    /// Answers a repo's webhooks for a token that reaches the repo and may
    /// manage them: the list, a new one, a change, a delete and a ping.
    /// Replies hide each secret, as GitHub does. None for any other path.
    fn repo_hooks(
        &mut self,
        method: &str,
        parts: &[&str],
        reach: Option<(&[String], Grants)>,
        body: &[u8],
    ) -> Option<(u16, Value)> {
        let ["repos", owner, name, "hooks", rest @ ..] = parts else {
            return None;
        };
        let missing = (404, json!({ "message": "Not Found" }));
        let Some((repos, grants)) = reach else {
            return Some((401, json!({ "message": "Bad credentials" })));
        };
        let repo = format!("{owner}/{name}");
        if !repos
            .iter()
            .any(|reached| reached.eq_ignore_ascii_case(&repo))
        {
            return Some(missing);
        }
        if !grants.hooks {
            return Some((
                403,
                json!({ "message": "Resource not accessible by integration" }),
            ));
        }
        let shown = |hook: &Value| {
            let mut hook = hook.clone();
            if hook["config"].get("secret").is_some() {
                hook["config"]["secret"] = json!("********");
            }
            hook
        };
        let asked: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let at = rest.first().and_then(|id| {
            let id: u64 = id.parse().ok()?;
            self.hooks.iter().position(|hook| {
                hook["id"] == json!(id)
                    && hook["repo"]
                        .as_str()
                        .is_some_and(|of| of.eq_ignore_ascii_case(&repo))
            })
        });
        Some(match (method, rest, at) {
            ("GET", [], _) => {
                let listed = self
                    .hooks
                    .iter()
                    .filter(|hook| {
                        hook["repo"]
                            .as_str()
                            .is_some_and(|of| of.eq_ignore_ascii_case(&repo))
                    })
                    .map(shown)
                    .collect();
                (200, Value::Array(listed))
            }
            ("POST", [], _) => {
                self.hook_ids += 1;
                let hook = json!({
                    "id": self.hook_ids,
                    "repo": repo,
                    "name": asked["name"],
                    "active": asked["active"],
                    "events": asked["events"],
                    "config": asked["config"],
                });
                self.hooks.push(hook.clone());
                (201, shown(&hook))
            }
            ("PATCH", [_], Some(at)) => {
                let hook = &mut self.hooks[at];
                for field in ["active", "events", "config"] {
                    if let Some(value) = asked.get(field) {
                        hook[field] = value.clone();
                    }
                }
                (200, shown(hook))
            }
            ("DELETE", [_], Some(at)) => {
                self.hooks.remove(at);
                (204, Value::Null)
            }
            ("POST", [_, "pings"], Some(at)) => {
                let id = self.hooks[at]["id"].as_u64().unwrap_or_default();
                self.pings.push(id);
                (204, Value::Null)
            }
            _ => missing,
        })
    }

    /// Answers the device flow, a refresh, and `GET /user`. None for any
    /// other path.
    fn sign_in(
        &mut self,
        method: &str,
        path: &str,
        auth: Option<&str>,
        form: &HashMap<String, String>,
    ) -> Option<(u16, Value)> {
        let field = |name: &str| form.get(name).map(String::as_str);
        let refusal = |error: &str| (200, json!({ "error": error }));
        let login = &mut self.login;
        Some(match (method, path) {
            ("POST", "/login/device/code") => {
                if field("client_id") != Some(CLIENT_ID) {
                    (404, json!({ "error": "Not Found" }))
                } else if !login.device_flow {
                    (
                        400,
                        json!({ "error": "device_flow_disabled", "error_description": "Device Flow must be explicitly enabled for this App" }),
                    )
                } else {
                    (
                        200,
                        json!({
                            "device_code": DEVICE_CODE,
                            "user_code": USER_CODE,
                            "verification_uri": "https://github.com/login/device",
                            "expires_in": 900,
                            "interval": 5,
                        }),
                    )
                }
            }
            ("POST", "/login/oauth/access_token") if field("client_id") != Some(CLIENT_ID) => {
                refusal("incorrect_client_credentials")
            }
            ("POST", "/login/oauth/access_token") => match field("grant_type") {
                Some("urn:ietf:params:oauth:grant-type:device_code") => {
                    if field("device_code") != Some(DEVICE_CODE) {
                        refusal("incorrect_device_code")
                    } else if let Some(answer) = login.polls.pop_front() {
                        refusal(answer)
                    } else {
                        (200, login.issue())
                    }
                }
                Some("refresh_token") if login.down => (502, json!({ "message": "Bad Gateway" })),
                Some("refresh_token") => {
                    if login.refresh.is_some() && field("refresh_token") == login.refresh.as_deref()
                    {
                        (200, login.issue())
                    } else {
                        refusal("bad_refresh_token")
                    }
                }
                _ => refusal("unsupported_grant_type"),
            },
            ("GET", "/user") => match auth {
                Some(token) if login.valid.contains(token) => {
                    (200, json!({ "login": LOGIN, "id": 583_231 }))
                }
                _ => (401, json!({ "message": "Bad credentials" })),
            },
            _ => return None,
        })
    }

    /// Whether `auth` is a JWT the App's key signed, naming the App, and
    /// lasting no longer than GitHub allows.
    fn signed(&self, auth: Option<&str>) -> bool {
        let Some(jwt) = auth else { return false };
        let parts: Vec<&str> = jwt.split('.').collect();
        let [header, claims, signature] = parts.as_slice() else {
            return false;
        };
        let decode = |part: &str| -> Option<Value> {
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).ok()?).ok()
        };
        let (Some(header), Some(claims), Ok(signature)) = (
            decode(header),
            decode(claims),
            URL_SAFE_NO_PAD.decode(signature),
        ) else {
            return false;
        };
        let signed = format!("{}.{}", parts[0], parts[1]);
        let key = UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, &self.public_der);
        let iat = claims["iat"].as_u64().unwrap_or(0);
        let exp = claims["exp"].as_u64().unwrap_or(0);
        header["alg"] == "RS256"
            && key.verify(signed.as_bytes(), &signature).is_ok()
            && claims["iss"].as_u64() == Some(self.app_id)
            && exp > iat
            && exp - iat <= 10 * 60
    }

    /// The installation that reaches `owner/name`.
    fn reaching(&self, owner: &str, name: &str) -> Option<&Install> {
        self.installations.iter().find(|install| {
            !install.suspended
                && install.owner.eq_ignore_ascii_case(owner)
                && install
                    .repos
                    .iter()
                    .any(|repo| repo.eq_ignore_ascii_case(name))
        })
    }

    fn mint(&mut self, id: u64, body: &[u8]) -> (u16, Value) {
        let Some(install) = self.installations.iter().find(|install| install.id == id) else {
            return (404, json!({ "message": "Not Found" }));
        };
        let Ok(asked) = serde_json::from_slice::<Value>(body) else {
            return (400, json!({ "message": "Problems parsing JSON" }));
        };
        let granted = |name: &str, level: &str| match name {
            "contents" | "pull_requests" => level == "write" || level == "read",
            "metadata" => level == "read",
            "checks" | "statuses" | "actions" => install.reads && level == "read",
            "repository_hooks" => install.hooks && (level == "write" || level == "read"),
            "issues" => match install.issues {
                Some("write") => level == "write" || level == "read",
                Some("read") => level == "read",
                _ => false,
            },
            _ => false,
        };
        let permissions = asked["permissions"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        if !permissions
            .iter()
            .all(|(name, level)| level.as_str().is_some_and(|level| granted(name, level)))
        {
            return (
                422,
                json!({ "message": "The permissions requested are not granted to this installation." }),
            );
        }
        let repos = match asked.get("repositories") {
            None => None,
            Some(Value::Array(names)) => {
                let mut found = Vec::new();
                for name in names {
                    let repo = name.as_str().and_then(|name| {
                        install
                            .repos
                            .iter()
                            .find(|repo| repo.eq_ignore_ascii_case(name))
                    });
                    match repo {
                        Some(repo) => found.push(json!({
                            "name": repo,
                            "full_name": format!("{}/{repo}", install.owner),
                        })),
                        None => {
                            return (
                                422,
                                json!({ "message": "There is at least one repository that does not exist or is not accessible to the parent installation." }),
                            )
                        }
                    }
                }
                Some(found)
            }
            Some(_) => return (422, json!({ "message": "Invalid request." })),
        };
        self.minted += 1;
        let token = format!("{TOKEN_PREFIX}{}", self.minted);
        let mut reply = json!({
            "token": token,
            "expires_at": self.expires_at,
            "permissions": permissions,
            "repository_selection": if repos.is_some() { "selected" } else { "all" },
        });
        match repos {
            Some(repos) => {
                if let [only] = repos.as_slice() {
                    let repo = only["full_name"].as_str().unwrap_or_default();
                    let grants = Grants {
                        issues: permissions.contains_key("issues"),
                        reads: permissions.contains_key("checks"),
                        hooks: permissions.contains_key("repository_hooks"),
                    };
                    self.repo_tokens
                        .insert(token, (repo.to_ascii_lowercase(), grants));
                }
                reply["repositories"] = Value::Array(repos);
            }
            None => {
                self.listing.insert(token, id);
            }
        }
        (201, reply)
    }

    /// Answers a merge or an approval made as `login`, the way GitHub
    /// does: a merge only at the head it names, unless a merge queue takes
    /// it. None for any other request.
    fn as_user(
        &mut self,
        method: &str,
        parts: &[&str],
        login: &str,
        body: &[u8],
    ) -> Option<(u16, Value)> {
        let ["repos", _, _, "pulls", number, what] = parts else {
            return None;
        };
        let number: u64 = number.parse().ok()?;
        let asked: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let forbidden = (
            403,
            json!({ "message": "Resource not accessible by personal access token" }),
        );
        let missing = (404, json!({ "message": "Not Found" }));
        let (writes, queues) = (self.users_write, self.queues_merges);
        let pull = self
            .pulls
            .iter_mut()
            .find(|pull| pull["number"].as_u64() == Some(number));
        Some(match (method, *what) {
            ("PUT", "merge") => {
                let Some(pull) = pull else {
                    return Some(missing);
                };
                if !writes {
                    return Some(forbidden);
                }
                if pull["head"]["sha"] != asked["sha"] {
                    return Some((
                        409,
                        json!({ "message": "Head branch was modified. Review and try the merge again." }),
                    ));
                }
                if !queues {
                    pull["state"] = json!("closed");
                    pull["merged_at"] = json!("2026-10-31T12:00:00Z");
                }
                (
                    200,
                    json!({ "merged": true, "message": format!("Merged by {login}") }),
                )
            }
            ("POST", "reviews") => {
                if pull.is_none() {
                    return Some(missing);
                }
                if !writes {
                    return Some(forbidden);
                }
                let review = json!({
                    "id": 1,
                    "user": { "login": login },
                    "state": "APPROVED",
                    "commit_id": asked["commit_id"],
                    "html_url": "https://github.com/owner/repo/pull/1#review",
                });
                (200, review)
            }
            _ => return None,
        })
    }

    /// One page of what installation `id` reaches.
    fn repositories(&self, id: u64, query: &str) -> Value {
        let install = self.installations.iter().find(|install| install.id == id);
        let all: Vec<Value> = install
            .map(|install| {
                install
                    .repos
                    .iter()
                    .map(|repo| json!({ "name": repo, "full_name": format!("{}/{repo}", install.owner) }))
                    .collect()
            })
            .unwrap_or_default();
        let per_page = query_number(query, "per_page").unwrap_or(30).max(1);
        let page = query_number(query, "page").unwrap_or(1).max(1);
        let start = ((page - 1) * per_page).min(all.len());
        let end = (start + per_page).min(all.len());
        json!({ "total_count": all.len(), "repositories": all[start..end] })
    }
}

impl Login {
    /// A new access token, with a refresh token in place of the last when
    /// tokens expire.
    fn issue(&mut self) -> Value {
        self.issued += 1;
        let access = format!("{USER_TOKEN_PREFIX}{}", self.issued);
        self.valid.insert(access.clone());
        let mut reply = json!({ "access_token": access, "token_type": "bearer", "scope": "" });
        if let Some(expires_in) = self.expires_in {
            let refresh = format!("{REFRESH_PREFIX}{}", self.issued);
            self.refresh = Some(refresh.clone());
            reply["expires_in"] = json!(expires_in);
            reply["refresh_token"] = json!(refresh);
            reply["refresh_token_expires_in"] = json!(self.refresh_expires_in);
        }
        reply
    }
}

fn query_number(query: &str, name: &str) -> Option<usize> {
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
        .and_then(|value| value.parse().ok())
}
