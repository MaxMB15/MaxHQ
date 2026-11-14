// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The GitHub App, per W-6. The device keeps the App's key and mints
//! one-hour installation tokens from it, each scoped to one repository
//! with `contents: write`, `pull_requests: write` and `metadata: read`,
//! plus issues at what the installation grants and checks, statuses and
//! actions where it grants reading them. Only those tokens leave
//! `maxd`. A check reads the App's name and the repositories it reaches
//! when the App is set, then every hour.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use maxhq_protocol::guest::TokenReply;
use maxhq_protocol::ids::{CredentialId, DeviceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{Credential, CredentialKind};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Notify;
use tracing::{info, warn};
use zeroize::Zeroizing;

use self::api::{Access, Installation, Paced, Reads, Scope};
pub use self::api::{Api, Fetched, GITHUB_API, GITHUB_LOGIN};
pub use self::gh::{Gh, TOKEN_STARTS};
pub use self::items::NOT_GITHUB;
use self::key::AppKey;
pub use self::user::UserToken;
use super::{refused, Secrets, GITHUB_APP_FILE, GITHUB_KEY_FILE};
use crate::observe::claude::time::parse_iso8601;

mod api;
mod cache;
#[cfg(test)]
pub(super) mod fake;
mod gh;
mod items;
pub(super) mod key;
mod pulls;
mod repo_hooks;
#[cfg(test)]
mod tests;
mod user;

/// A token is minted again this long before it stops working.
const MARGIN: Duration = Duration::from_secs(5 * 60);
/// How often the check runs while it works.
const RECHECK: Duration = Duration::from_secs(60 * 60);
/// How soon the check runs again after it failed.
const RETRY: Duration = Duration::from_secs(5 * 60);
/// What follows a repository's name in the cache for its webhooks token.
const HOOKS: &str = "#hooks";

/// A repository on GitHub. Two names are the same repository whatever
/// their case, as on GitHub.
#[derive(Clone, Debug)]
pub struct RepoName {
    owner: String,
    name: String,
}

impl RepoName {
    /// Reads "owner/name", or a GitHub clone URL such as
    /// `https://github.com/owner/name.git` or `git@github.com:owner/name`.
    /// Returns none for anything else, including a URL for another host or
    /// one carrying a login.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let path = [
            "https://github.com/",
            "ssh://git@github.com/",
            "git@github.com:",
        ]
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix))
        .unwrap_or(text);
        let path = path.strip_suffix('/').unwrap_or(path);
        let path = path.strip_suffix(".git").unwrap_or(path);
        let (owner, name) = path.split_once('/')?;
        let owner_ok = (1..=39).contains(&owner.len())
            && owner
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-');
        let name_ok = (1..=100).contains(&name.len())
            && name != "."
            && name != ".."
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
        (owner_ok && name_ok).then(|| Self {
            owner: owner.to_owned(),
            name: name.to_owned(),
        })
    }

    /// The account that owns it.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Its name within the account.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The name it's cached and looked up by, the same for any case.
    pub fn key(&self) -> String {
        self.to_string().to_ascii_lowercase()
    }
}

impl PartialEq for RepoName {
    fn eq(&self, other: &Self) -> bool {
        self.owner.eq_ignore_ascii_case(&other.owner) && self.name.eq_ignore_ascii_case(&other.name)
    }
}

impl Eq for RepoName {}

impl fmt::Display for RepoName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// An installation token for one repository.
#[derive(Clone, Debug)]
pub struct GithubToken {
    /// The token, for `git` as the password with any user name.
    pub value: Secret,
    /// When it stops working.
    pub expires_at: Timestamp,
}

impl From<GithubToken> for TokenReply {
    fn from(token: GithubToken) -> Self {
        TokenReply::Token {
            value: token.value,
            expires_at: token.expires_at,
        }
    }
}

/// Why there's no token.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum GithubError {
    /// The device has no App.
    #[error("This device has no GitHub App. Set one up in Settings.")]
    NoApp,
    /// What was asked for isn't a GitHub repository.
    #[error("That isn't a GitHub repository.")]
    BadRepo,
    /// The App isn't installed where the repository is.
    #[error(
        "The GitHub App isn't installed on {0}. Install it there from the App's page on GitHub."
    )]
    NotInstalled(String),
    /// The App's installation hasn't granted managing webhooks, so GitHub
    /// wouldn't mint a token for them.
    #[error("The GitHub App may not manage this repository's webhooks: {0}")]
    HooksNotGranted(String),
    /// GitHub said no.
    #[error("GitHub refused with {status}: {message}")]
    Refused {
        /// The HTTP status.
        status: u16,
        /// GitHub's message.
        message: String,
    },
    /// GitHub couldn't be reached.
    #[error("This device couldn't reach GitHub: {0}")]
    Unreachable(String),
    /// GitHub's reply wasn't what its API promises.
    #[error("GitHub's reply wasn't what this device expected.")]
    BadReply,
    /// The key wouldn't sign.
    #[error("The App's key couldn't sign a token.")]
    BadKey,
    /// There's no `gh` to read with.
    #[error("gh isn't installed on this device.")]
    NoGh,
    /// `gh` has no sign-in for github.com, or its sign-in stopped working.
    #[error("gh isn't signed in on this device. Run gh auth login on it.")]
    GhSignedOut,
    /// `gh` ran but couldn't read from GitHub.
    #[error("gh couldn't read from GitHub: {0}")]
    GhFailed(String),
    /// The device couldn't make or keep a repository webhook's signing
    /// secret.
    #[error("This device couldn't keep a webhook's secret: {0}")]
    NoSecret(String),
    /// No one has signed in to GitHub on this device, so it can't merge or
    /// approve as you.
    #[error("Sign in to GitHub in Settings to merge and approve here.")]
    NotSignedIn,
}

impl From<GithubError> for TokenReply {
    fn from(err: GithubError) -> Self {
        TokenReply::Refused {
            reason: err.to_string(),
        }
    }
}

/// The App and the tokens minted from it.
pub struct Github {
    api: Api,
    /// What the `#` lookup reads with where the App can't.
    gh: Gh,
    /// The App, while the device has one.
    app: Mutex<Option<Arc<App>>>,
    /// Tokens by lowercase "owner/name", with [`HOOKS`] after the name
    /// for one that manages its webhooks.
    cache: Mutex<HashMap<String, Cached>>,
    /// Held while minting, so callers asking at once share one token.
    minting: tokio::sync::Mutex<()>,
    /// Starts a check now.
    wake: Notify,
    /// Tells each App set apart from the one before.
    generations: AtomicU64,
    /// Your own sign-in.
    pub(super) user: user::SignIn,
    /// What was read from GitHub lately.
    cache_reads: Mutex<cache::Cache>,
    /// When each `#` search in the last minute went out.
    searches: Mutex<VecDeque<Timestamp>>,
    /// How fast the App may read each repo, by lowercase "owner/name", as
    /// its last reply said.
    pacing: Mutex<HashMap<String, Paced>>,
}

impl Github {
    /// No App yet, with GitHub's API at `api` and `gh` for the lookup.
    pub fn new(api: Api, gh: Gh) -> Self {
        Self {
            api,
            gh,
            app: Mutex::new(None),
            cache: Mutex::new(HashMap::new()),
            minting: tokio::sync::Mutex::new(()),
            wake: Notify::new(),
            generations: AtomicU64::new(0),
            user: user::SignIn::default(),
            cache_reads: Mutex::new(cache::Cache::default()),
            searches: Mutex::new(VecDeque::new()),
            pacing: Mutex::new(HashMap::new()),
        }
    }

    fn current(&self) -> Option<Arc<App>> {
        lock(&self.app).clone()
    }

    /// The cached token under `key`, while it has more than [`MARGIN`]
    /// left.
    fn cached(&self, app: &App, key: &str, now: Timestamp) -> Option<GithubToken> {
        let cache = lock(&self.cache);
        let cached = cache.get(key)?;
        let fresh =
            cached.generation == app.generation && now.after(MARGIN) < cached.token.expires_at;
        fresh.then(|| cached.token.clone())
    }
}

/// The App the device holds.
struct App {
    generation: u64,
    app_id: u64,
    key: AppKey,
    stored_at: Timestamp,
    found: Mutex<Found>,
}

impl App {
    fn jwt(&self, now: Timestamp) -> Result<Secret, GithubError> {
        self.key
            .jwt(self.app_id, now)
            .map_err(|_| GithubError::BadKey)
    }

    fn credential(&self, device: DeviceId, found: Found) -> Credential {
        Credential {
            id: github_credential(device),
            device,
            kind: CredentialKind::GithubApp {
                app_id: self.app_id,
                slug: found.slug,
                key_sha256: self.key.sha256().to_owned(),
                repos: found.repos,
            },
            stored_at: self.stored_at,
            problem: found.problem,
        }
    }
}

/// What the check found.
#[derive(Clone, Default)]
struct Found {
    slug: Option<String>,
    /// The organization that owns the App, if one does.
    org: Option<String>,
    /// The App's public client ID.
    client_id: Option<String>,
    /// "owner/name", sorted.
    repos: Vec<String>,
    /// The installation that reaches each repository, by lowercase
    /// "owner/name".
    installations: HashMap<String, Reach>,
    problem: Option<String>,
}

/// The installation that reaches a repository.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Reach {
    id: u64,
    /// What it grants on issues. An App made before MaxHQ asked for issues
    /// has none, and an account has to accept them once the App adds them.
    issues: Option<Access>,
    /// The checks, statuses and actions it lets the PR viewer read.
    reads: Reads,
}

impl Reach {
    fn of(installation: &Installation) -> Self {
        Self {
            id: installation.id,
            issues: installation.permissions.issues(),
            reads: installation.permissions.reads(),
        }
    }
}

/// A token kept for reuse.
struct Cached {
    generation: u64,
    token: GithubToken,
}

/// `github-app.json`.
#[derive(Serialize, Deserialize)]
struct AppFile {
    app_id: u64,
    key_sha256: String,
    stored_at: Timestamp,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    repos: Vec<String>,
}

/// "a", "a and b", or "a, b and c".
fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The App's credential on `device`.
fn github_credential(device: DeviceId) -> CredentialId {
    CredentialId::derive(&format!("github-app:{device}"))
}

/// What a token for `repo` is cached under, for its webhooks alone with
/// `hooks`.
fn cache_key(repo: &RepoName, hooks: bool) -> String {
    if hooks {
        format!("{}{HOOKS}", repo.key())
    } else {
        repo.key()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Secrets {
    /// Answers `SetGithubApp`: checks the key is RSA and signs with it,
    /// stores it, and starts the check. A new App replaces the old one.
    pub(super) fn set_github_app(&self, app_id: u64, private_key: Secret) -> ActionOutcome {
        if app_id == 0 {
            return refused("Enter the App ID from the top of the App's page on GitHub.");
        }
        let pem = Zeroizing::new(format!("{}\n", private_key.expose().trim()));
        drop(private_key);
        let key = match AppKey::parse(&pem) {
            Ok(key) => key,
            Err(err) => return refused(err.to_string()),
        };
        if let Err(err) = key.jwt(app_id, self.now()) {
            return refused(err.to_string());
        }
        let stored_at = self.now();
        let file = AppFile {
            app_id,
            key_sha256: key.sha256().to_owned(),
            stored_at,
            slug: None,
            client_id: None,
            repos: Vec::new(),
        };
        let Ok(json) = serde_json::to_vec_pretty(&file) else {
            return refused("This device couldn't store the key.");
        };

        let github = &self.inner.github;
        let folder = &self.inner.folder;
        let mut current = lock(&github.app);
        if let Err(err) = folder.write(GITHUB_KEY_FILE, pem.as_bytes()) {
            warn!("couldn't store the GitHub App's key: {err}");
            return refused(format!("This device couldn't store the key: {err}"));
        }
        if let Err(err) = folder.write(GITHUB_APP_FILE, &json) {
            // The new key sits next to the old App's file now, so neither
            // stays.
            warn!("couldn't store the GitHub App's ID: {err}");
            let _ = folder.remove(GITHUB_KEY_FILE);
            let _ = folder.remove(GITHUB_APP_FILE);
            if current.take().is_some() {
                lock(&github.cache).clear();
                self.inner.hub.apply([Change::Remove {
                    key: EntityKey::Credential(github_credential(self.device())),
                }]);
            }
            return refused(format!("This device couldn't store the key: {err}"));
        }

        info!(
            app_id,
            key_sha256 = key.sha256(),
            "stored the GitHub App's key"
        );
        let app = Arc::new(App {
            generation: github.generations.fetch_add(1, Ordering::Relaxed) + 1,
            app_id,
            key,
            stored_at,
            found: Mutex::new(Found::default()),
        });
        *current = Some(app.clone());
        lock(&github.cache).clear();
        let credential = app.credential(self.device(), Found::default());
        self.inner.hub.apply([Change::Upsert {
            value: Entity::Credential(credential),
        }]);
        drop(current);
        github.wake.notify_one();
        ActionOutcome::Done
    }

    /// Reads the App the folder holds, if any, and publishes it. The next
    /// check fills in what it reaches.
    pub(super) fn load_github(&self) {
        let folder = &self.inner.folder;
        let (file, pem) = match (folder.read(GITHUB_APP_FILE), folder.read(GITHUB_KEY_FILE)) {
            (Ok(file), Ok(pem)) => (file, pem),
            (Err(err), _) | (_, Err(err)) => {
                warn!("couldn't read the GitHub App: {err}");
                return;
            }
        };
        let (file, pem) = match (file, pem) {
            (Some(file), Some(pem)) => (file, pem),
            (None, None) => return,
            _ => {
                // A crash between the two writes leaves one alone.
                warn!("removed half a GitHub App, so set it again");
                let _ = folder.remove(GITHUB_KEY_FILE);
                let _ = folder.remove(GITHUB_APP_FILE);
                return;
            }
        };
        let Ok(file) = serde_json::from_str::<AppFile>(file.expose()) else {
            warn!("skipped the GitHub App: {GITHUB_APP_FILE} doesn't describe one");
            return;
        };
        let key = match AppKey::parse(pem.expose()) {
            Ok(key) => key,
            Err(err) => {
                warn!(app_id = file.app_id, "skipped the GitHub App's key: {err}");
                return;
            }
        };
        if key.sha256() != file.key_sha256 {
            warn!(
                app_id = file.app_id,
                "skipped the GitHub App: its key isn't the one stored with it"
            );
            return;
        }

        info!(
            app_id = file.app_id,
            key_sha256 = key.sha256(),
            "loaded the GitHub App"
        );
        let found = Found {
            slug: file.slug,
            client_id: file.client_id,
            repos: file.repos,
            ..Found::default()
        };
        let github = &self.inner.github;
        let app = Arc::new(App {
            generation: github.generations.fetch_add(1, Ordering::Relaxed) + 1,
            app_id: file.app_id,
            key,
            stored_at: file.stored_at,
            found: Mutex::new(found.clone()),
        });
        let credential = app.credential(self.device(), found);
        *lock(&github.app) = Some(app);
        self.inner.hub.apply([Change::Upsert {
            value: Entity::Credential(credential),
        }]);
    }

    /// Answers `CheckGithubApp`: wakes the background check, which then
    /// publishes the App with what it found.
    pub(super) fn check_github_app(&self) -> ActionOutcome {
        let github = &self.inner.github;
        if github.current().is_none() {
            return refused("This device doesn't hold a GitHub App, so there's nothing to check.");
        }
        github.wake.notify_one();
        ActionOutcome::Done
    }

    /// Forgets the App behind `credential`, deleting its key and its file.
    /// Returns none when `credential` isn't the App's.
    pub(super) fn forget_github(&self, credential: CredentialId) -> Option<ActionOutcome> {
        if credential != github_credential(self.device()) {
            return None;
        }
        let github = &self.inner.github;
        let mut current = lock(&github.app);
        let app_id = current.as_ref()?.app_id;
        if let Err(err) = self.inner.folder.remove(GITHUB_KEY_FILE) {
            warn!(app_id, "couldn't delete the GitHub App's key: {err}");
            return Some(refused(format!(
                "This device couldn't delete the key: {err}"
            )));
        }
        if let Err(err) = self.inner.folder.remove(GITHUB_APP_FILE) {
            // The key is gone, which is what counts. The next start clears
            // the file.
            warn!(app_id, "couldn't delete {GITHUB_APP_FILE}: {err}");
        }
        *current = None;
        self.revoke_kept_tokens();
        self.inner.hub.apply([Change::Remove {
            key: EntityKey::Credential(credential),
        }]);
        info!(app_id, "forgot the GitHub App");
        Some(ActionOutcome::Done)
    }

    /// Empties the token cache and asks GitHub to end those tokens, so an
    /// agent holding one loses it at sign-out rather than within the hour.
    /// This doesn't wait for GitHub, and signing out offline still works:
    /// a token GitHub never heard about just runs to its expiry.
    fn revoke_kept_tokens(&self) {
        let tokens: Vec<Secret> = lock(&self.inner.github.cache)
            .drain()
            .map(|(_, cached)| cached.token.value)
            .collect();
        if tokens.is_empty() {
            return;
        }
        let secrets = self.clone();
        tokio::spawn(async move {
            for token in tokens {
                let api = &secrets.inner.github.api;
                if let Err(err) = api.revoke_installation_token(&token).await {
                    warn!("couldn't revoke a GitHub token at sign-out: {err}");
                }
            }
        });
    }

    /// An installation token that reaches `repo` alone, with W-6's
    /// permissions. A token is reused until [`MARGIN`] before it expires.
    pub async fn github(&self, repo: &str) -> Result<GithubToken, GithubError> {
        let repo = RepoName::parse(repo).ok_or(GithubError::BadRepo)?;
        self.token(&repo, false).await
    }

    /// An installation token that reaches `repo` alone, with W-6's
    /// permissions, or with `hooks` only to manage its webhooks.
    async fn token(&self, repo: &RepoName, hooks: bool) -> Result<GithubToken, GithubError> {
        self.kept_token(repo, hooks).await.map(|(token, _)| token)
    }

    /// [`Secrets::token`], and whether it was kept from before rather than
    /// minted now.
    async fn kept_token(
        &self,
        repo: &RepoName,
        hooks: bool,
    ) -> Result<(GithubToken, bool), GithubError> {
        let github = &self.inner.github;
        let app = github.current().ok_or(GithubError::NoApp)?;
        let key = cache_key(repo, hooks);
        if let Some(token) = github.cached(&app, &key, self.now()) {
            return Ok((token, true));
        }
        let _minting = github.minting.lock().await;
        if let Some(token) = github.cached(&app, &key, self.now()) {
            return Ok((token, true));
        }
        let minted = self.mint(&app, repo, hooks).await;
        self.note(&app, &minted);
        let token = minted?;
        info!(app_id = app.app_id, %repo, hooks, "minted a GitHub token");
        lock(&github.cache).insert(
            key,
            Cached {
                generation: app.generation,
                token: token.clone(),
            },
        );
        Ok((token, false))
    }

    /// Stops reusing `token`, which [`Secrets::token`] gave for `repo` and
    /// `hooks`, so the next ask mints another. One minted since stays.
    fn drop_token(&self, repo: &RepoName, hooks: bool, token: &GithubToken) {
        let mut cache = lock(&self.inner.github.cache);
        let key = cache_key(repo, hooks);
        if cache
            .get(&key)
            .is_some_and(|cached| cached.token.value == token.value)
        {
            cache.remove(&key);
        }
    }

    /// `GET path` on GitHub's API with a token for `repo`, asking only for
    /// a change since `etag`, and how long GitHub asks a poller to wait.
    pub async fn github_poll<T: DeserializeOwned>(
        &self,
        repo: &RepoName,
        path: &str,
        etag: Option<&str>,
    ) -> Result<(Fetched<T>, Option<Duration>), GithubError> {
        let token = self.github(&repo.to_string()).await?;
        let api = &self.inner.github.api;
        api.get_polled(&token.value, path, etag).await
    }

    /// Mints a token for `repo` from the installation that reaches it, for
    /// its webhooks alone with `hooks`.
    async fn mint(
        &self,
        app: &App,
        repo: &RepoName,
        hooks: bool,
    ) -> Result<GithubToken, GithubError> {
        let api = &self.inner.github.api;
        let jwt = app.jwt(self.now())?;
        let known = lock(&app.found).installations.get(&repo.key()).copied();
        let reach = match known {
            Some(reach) => reach,
            None => api
                .repo_installation(&jwt, repo)
                .await?
                .map(|installation| Reach::of(&installation))
                .ok_or_else(|| GithubError::NotInstalled(repo.to_string()))?,
        };
        let scope = if hooks {
            Scope::Hooks { name: repo.name() }
        } else {
            Scope::Repo {
                name: repo.name(),
                issues: reach.issues,
                reads: reach.reads,
            }
        };
        let minted = match api.mint(&jwt, reach.id, scope).await {
            Err(GithubError::Refused { status: 404, .. }) => {
                // The installation went away. Ask GitHub afresh next time.
                lock(&app.found).installations.remove(&repo.key());
                return Err(GithubError::NotInstalled(repo.to_string()));
            }
            minted => minted?,
        };
        let reaches = match minted.repositories.as_deref() {
            Some([only]) => RepoName::parse(&only.full_name).as_ref() == Some(repo),
            _ => false,
        };
        if !reaches {
            return Err(GithubError::BadReply);
        }
        let expires_at = parse_iso8601(&minted.expires_at).ok_or(GithubError::BadReply)?;
        Ok(GithubToken {
            value: minted.token,
            expires_at,
        })
    }

    /// Shows a refused key on the App's credential, and clears it once a
    /// token comes back.
    fn note(&self, app: &App, minted: &Result<GithubToken, GithubError>) {
        let problem = match minted {
            Ok(_) => None,
            Err(GithubError::Refused {
                status: 401,
                message,
            }) => Some(format!("GitHub refused the App's key: {message}")),
            Err(_) => return,
        };
        {
            let mut found = lock(&app.found);
            if found.problem == problem {
                return;
            }
            found.problem = problem;
        }
        self.publish_app(app, false);
    }

    /// Checks the App in the background: when it's set, when a client asks,
    /// and every hour. Each check also keeps your sign-in from lapsing.
    pub async fn run(self) {
        loop {
            self.keep_github_user().await;
            let wait = if self.check_github().await {
                RECHECK
            } else {
                RETRY
            };
            tokio::select! {
                () = self.inner.github.wake.notified() => {}
                () = tokio::time::sleep(wait) => {}
            }
        }
    }

    /// Asks GitHub for the App's name and every repository it reaches, then
    /// mints a token for the first, as the setup's last step says. Returns
    /// whether all of it worked, issues included.
    pub(super) async fn check_github(&self) -> bool {
        let Some(app) = self.inner.github.current() else {
            return true;
        };
        let problem = match self.look(&app).await {
            Ok((seen, no_issues)) => {
                let first = seen.repos.first().cloned();
                {
                    let mut found = lock(&app.found);
                    // A token minted before an account granted issues has
                    // none, so mint again where that changed.
                    lock(&self.inner.github.cache).retain(|key, _| {
                        let repo = key.trim_end_matches(HOOKS);
                        found.installations.get(repo) == seen.installations.get(repo)
                    });
                    found.slug = seen.slug;
                    found.org = seen.org;
                    found.client_id = seen.client_id;
                    found.repos = seen.repos;
                    found.installations = seen.installations;
                }
                match first {
                    None => Some(
                        "The App isn't installed on any repositories yet. Install it from its page on GitHub."
                            .to_owned(),
                    ),
                    Some(repo) => match self.github(&repo).await {
                        Ok(_) => no_issues,
                        Err(err) => Some(format!("GitHub wouldn't give a token for {repo}: {err}")),
                    },
                }
            }
            Err(err) => Some(err.to_string()),
        };
        match &problem {
            None => info!(app_id = app.app_id, "checked the GitHub App"),
            Some(problem) => warn!(
                app_id = app.app_id,
                "the GitHub App's check failed: {problem}"
            ),
        }
        let worked = problem.is_none();
        lock(&app.found).problem = problem;
        self.publish_app(&app, true);
        worked
    }

    /// What the App is and what it reaches, from GitHub, and what to tell
    /// you when it can't reach issues.
    async fn look(&self, app: &App) -> Result<(Found, Option<String>), GithubError> {
        let api = &self.inner.github.api;
        let jwt = app.jwt(self.now())?;
        let info = api.app(&jwt).await?;
        let mut seen = Found {
            slug: info.slug,
            org: info
                .owner
                .filter(|owner| owner.kind == "Organization")
                .map(|owner| owner.login),
            client_id: info.client_id,
            ..Found::default()
        };
        let mut without_issues = Vec::new();
        for installation in api.installations(&jwt).await? {
            if installation.suspended_at.is_some() {
                continue;
            }
            let reach = Reach::of(&installation);
            if reach.issues.is_none() {
                without_issues.push(match &installation.account {
                    Some(account) => account.login.clone(),
                    None => format!("installation {}", installation.id),
                });
            }
            // The list can take a while, so each installation gets a JWT of
            // its own.
            let jwt = app.jwt(self.now())?;
            let listing = api.mint(&jwt, installation.id, Scope::Listing).await?;
            for name in api.installation_repos(&listing.token).await? {
                let Some(repo) = RepoName::parse(&name) else {
                    continue;
                };
                if seen.installations.insert(repo.key(), reach).is_none() {
                    seen.repos.push(repo.to_string());
                }
            }
        }
        seen.repos.sort_by_key(|repo| repo.to_ascii_lowercase());
        let no_issues = if info.permissions.issues().is_none() {
            Some(
                "The App can't reach issues. On its GitHub page, open Permissions & events, set Issues to \"Read and write\", and save. Then accept the change where it's installed."
                    .to_owned(),
            )
        } else if without_issues.is_empty() {
            None
        } else {
            Some(format!(
                "The App can't reach issues on {} until the new permission is accepted there. On GitHub, open Settings, Applications, Installed GitHub Apps, click Configure next to the App, and accept the request.",
                listed(&without_issues)
            ))
        };
        Ok((seen, no_issues))
    }

    /// Publishes the App's credential, and with `save` writes what the
    /// check found to its file. Does nothing once the App was replaced or
    /// forgotten.
    fn publish_app(&self, app: &App, save: bool) {
        let current = lock(&self.inner.github.app);
        if current.as_ref().map(|current| current.generation) != Some(app.generation) {
            return;
        }
        let found = lock(&app.found).clone();
        if save {
            let file = AppFile {
                app_id: app.app_id,
                key_sha256: app.key.sha256().to_owned(),
                stored_at: app.stored_at,
                slug: found.slug.clone(),
                client_id: found.client_id.clone(),
                repos: found.repos.clone(),
            };
            let written = serde_json::to_vec_pretty(&file)
                .map_err(std::io::Error::other)
                .and_then(|json| self.inner.folder.write(GITHUB_APP_FILE, &json));
            if let Err(err) = written {
                warn!(
                    app_id = app.app_id,
                    "couldn't save what the GitHub App reaches: {err}"
                );
            }
        }
        let credential = app.credential(self.device(), found);
        self.inner.hub.apply([Change::Upsert {
            value: Entity::Credential(credential),
        }]);
    }
}
