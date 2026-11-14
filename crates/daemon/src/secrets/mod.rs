// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The device's secret store: Claude setup-tokens, the ChatGPT sign-in, a
//! Cursor API key, the GitHub App's key and the values of local ACP
//! agents' variables, each in a 0600 file under a 0700 folder, written
//! by fsync and rename. Values stay in [`Secret`] and never reach a log.
//! It serves [`Credentials`](crate::session::holders::Credentials) to the
//! session manager and mints GitHub tokens for the guest link and the push
//! preflight through [`github`].
//!
//! Requests from clients arrive through the free functions here, which
//! reach the store [`install`] put in place. Until then they're refused
//! with "not yet". Codex in a container gets its access token from the
//! ChatGPT sign-in through [`codex_token`]. Signing a Claude Code, Codex or
//! Cursor login in again runs the CLI's own sign-in through `cli_login`, and
//! the CLI keeps the login.

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use maxhq_protocol::acp::AcpEnvValue;
use maxhq_protocol::find::GitItemSearch;
use maxhq_protocol::guest::TokenReply;
use maxhq_protocol::ids::{AccountId, AcpAgentId, CredentialId, DeviceId, SignInId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{Account, Credential, LoginKind, Vendor};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use serde::de::DeserializeOwned;

use crate::hub::Hub;
use crate::pulls::source::PullSource;
use crate::server::not_yet;
use crate::session::holders::Credentials;

mod acp;
pub(crate) mod apns;
mod chatgpt;
mod claude;
mod cli_login;
mod cursor;
pub(crate) mod files;
mod github;
mod hooks;
pub(crate) mod http;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;

pub use apns::ApnsKey;
pub use github::{
    Fetched, GithubError, GithubToken, RepoName, UserToken, NOT_GITHUB, TOKEN_STARTS,
};

/// The store's folder under `maxd`'s data folder.
pub const DIR: &str = "secrets";
/// The ChatGPT sign-in's file.
pub const CHATGPT_FILE: &str = "chatgpt.json";
/// The Cursor API key's file.
pub const CURSOR_FILE: &str = "cursor.json";
/// The GitHub App's private key.
pub const GITHUB_KEY_FILE: &str = "github-app.pem";
/// The GitHub App's ID and what its installations reach.
pub const GITHUB_APP_FILE: &str = "github-app.json";
/// Your own GitHub sign-in.
pub const GITHUB_USER_FILE: &str = "github-user.json";

/// The file for the setup-token minted on the Mac account `origin`.
pub fn claude_token_file(origin: AccountId) -> String {
    format!("claude-{origin}.token")
}

/// The longest name a credential takes.
const MAX_LABEL: usize = 200;

/// The clock the store reads. Tests bring their own.
pub type Now = Arc<dyn Fn() -> Timestamp + Send + Sync>;

/// The store the free functions reach, once `maxd` has opened it.
static STORE: OnceLock<Secrets> = OnceLock::new();

/// The device's secret store. Clones share it.
#[derive(Clone)]
pub struct Secrets {
    inner: Arc<Inner>,
}

struct Inner {
    hub: Hub,
    folder: files::Folder,
    now: Now,
    claude: claude::Tokens,
    cursor: cursor::Key,
    chatgpt: chatgpt::ChatGpt,
    github: github::Github,
    acp: acp::Values,
    apns: apns::Held,
    cli_login: cli_login::Rounds,
}

/// A stored login's account, before its CLI says more about it.
fn login_account(
    id: AccountId,
    device: DeviceId,
    vendor: Vendor,
    label: String,
    login: LoginKind,
    config_dir: String,
) -> Account {
    Account {
        id,
        device,
        vendor,
        label,
        login,
        origin: None,
        config_dir,
        email: None,
        organization: None,
        plan: None,
        models: Vec::new(),
        usage: None,
        usage_failure: None,
        acp: None,
    }
}

impl Secrets {
    /// Opens the store in `path`, making the folder if it's missing, and
    /// publishes the credentials it holds.
    pub fn open(path: PathBuf, hub: Hub) -> io::Result<Self> {
        let api = github::Api::new(github::GITHUB_API, github::GITHUB_LOGIN);
        let gh = github::Gh::installed();
        let issuer = chatgpt::Issuer::new(chatgpt::OPENAI_ISSUER);
        Self::open_with(path, hub, api, gh, issuer, Arc::new(Timestamp::now))
    }

    /// Opens the store with its own GitHub API, `gh`, ChatGPT sign-in
    /// server and clock.
    fn open_with(
        path: PathBuf,
        hub: Hub,
        api: github::Api,
        gh: github::Gh,
        issuer: chatgpt::Issuer,
        now: Now,
    ) -> io::Result<Self> {
        let folder = files::Folder::open(path)?;
        folder.clear_leftovers();
        let chatgpt = chatgpt::ChatGpt::open(issuer, &folder);
        let cli_login = cli_login::Rounds::new(hub.clone(), cli_login::Limits::default());
        let secrets = Self {
            inner: Arc::new(Inner {
                hub,
                folder,
                now,
                claude: claude::Tokens::default(),
                cursor: cursor::Key::default(),
                chatgpt,
                github: github::Github::new(api, gh),
                acp: acp::Values::default(),
                apns: apns::Held::default(),
                cli_login,
            }),
        };
        secrets.load_claude();
        secrets.load_cursor();
        secrets.load_chatgpt();
        secrets.load_github();
        secrets.load_github_user();
        secrets.load_acp();
        secrets.load_apns();
        Ok(secrets)
    }

    fn now(&self) -> Timestamp {
        (self.inner.now)()
    }

    /// Runs `then` with each account you sign in to again through its CLI,
    /// once the CLI stored the new login. Only the first call counts.
    pub fn on_signed_in(&self, then: impl Fn(AccountId) + Send + Sync + 'static) {
        self.inner.cli_login.on_signed_in(Box::new(then));
    }

    fn device(&self) -> DeviceId {
        self.inner.hub.device().id
    }

    /// Shows a login: its credential, and the account agents run as.
    fn publish(&self, credential: Credential, account: Account) {
        self.inner.hub.apply([
            Change::Upsert {
                value: Entity::Credential(credential),
            },
            Change::Upsert {
                value: Entity::Account(account),
            },
        ]);
    }

    /// Takes a login's credential and account away, with the models its
    /// CLI listed.
    fn unpublish(&self, credential: CredentialId, account: AccountId) {
        self.inner.hub.forget_models(account);
        self.inner.hub.apply([
            Change::Remove {
                key: EntityKey::Credential(credential),
            },
            Change::Remove {
                key: EntityKey::Account(account),
            },
        ]);
    }

    /// Answers `ForgetCredential`, for whichever kind it is. Signing out
    /// of ChatGPT waits on OpenAI, and the files go off the async threads.
    async fn forget(&self, credential: CredentialId) -> ActionOutcome {
        if let Some(outcome) = self.forget_chatgpt(credential).await {
            return outcome;
        }
        let secrets = self.clone();
        tokio::task::spawn_blocking(move || secrets.forget_file(credential))
            .await
            .unwrap_or_else(|_| refused("This device couldn't forget the credential."))
    }

    fn forget_file(&self, credential: CredentialId) -> ActionOutcome {
        if let Some(outcome) = self.forget_claude(credential) {
            return outcome;
        }
        if let Some(outcome) = self.forget_cursor(credential) {
            return outcome;
        }
        if let Some(outcome) = self.forget_github(credential) {
            return outcome;
        }
        if let Some(outcome) = self.forget_github_user(credential) {
            return outcome;
        }
        if let Some(outcome) = self.forget_apns(credential) {
            return outcome;
        }
        refused("This device doesn't hold that credential.")
    }
}

impl Credentials for Secrets {
    fn claude_token(&self, account: AccountId) -> Option<Secret> {
        self.setup_token(account)
    }

    fn holds_claude_token(&self) -> bool {
        self.holds_setup_token()
    }

    fn has_chatgpt(&self, account: AccountId) -> bool {
        self.has_signed_in(account)
    }

    fn cursor_key(&self, account: AccountId) -> Option<Secret> {
        self.api_key(account)
    }

    fn acp_env(&self, agent: AcpAgentId) -> Vec<AcpEnvValue> {
        self.acp_values(agent)
    }
}

/// Makes `secrets` the store the free functions reach. Only the first call
/// counts, since `maxd` opens one store.
pub fn install(secrets: Secrets) {
    let _ = STORE.set(secrets);
}

/// Answers `StoreClaudeToken`.
pub fn store_claude_token(origin: AccountId, label: String, token: Secret) -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.store_claude_token(origin, label, token),
        None => not_yet(),
    }
}

/// Answers `StoreCursorKey`.
pub fn store_cursor_key(label: String, key: Secret) -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.store_cursor_key(label, key),
        None => not_yet(),
    }
}

/// Answers `BeginChatGptSignIn`.
pub async fn begin_chatgpt_sign_in() -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.begin_chatgpt().await,
        None => not_yet(),
    }
}

/// Answers `FinishChatGptSignIn`.
pub async fn finish_chatgpt_sign_in(
    sign_in: SignInId,
    code: Secret,
    state: String,
    client_id: Option<String>,
) -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.finish_chatgpt(sign_in, code, state, client_id).await,
        None => not_yet(),
    }
}

/// Answers `SetGithubApp`.
pub fn set_github_app(app_id: u64, private_key: Secret) -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.set_github_app(app_id, private_key),
        None => not_yet(),
    }
}

/// Answers `SetApnsKey`.
pub fn set_apns_key(key_id: &str, team_id: &str, private_key: Secret) -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.set_apns_key(key_id, team_id, private_key),
        None => not_yet(),
    }
}

/// The APNs key pushes sign with, while this device holds one.
pub fn apns_key() -> Option<Arc<ApnsKey>> {
    STORE.get().and_then(Secrets::apns)
}

/// Answers `CheckGithubApp`.
pub fn check_github_app() -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.check_github_app(),
        None => not_yet(),
    }
}

/// Answers `BeginGithubSignIn`.
pub async fn begin_github_sign_in(client_id: Option<String>) -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.begin_github_user(client_id).await,
        None => not_yet(),
    }
}

/// Answers `CancelGithubSignIn`.
pub fn cancel_github_sign_in(sign_in: SignInId) -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.cancel_github_user(sign_in),
        None => not_yet(),
    }
}

/// Answers `CheckGithubSignIn`.
pub fn check_github_sign_in(sign_in: SignInId) -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.check_github_user(sign_in),
        None => not_yet(),
    }
}

/// Answers `BeginCliLogin`.
pub async fn begin_cli_login(account: AccountId, device_code: bool) -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.inner.cli_login.begin(account, device_code).await,
        None => not_yet(),
    }
}

/// Answers `FinishCliLogin`.
pub async fn finish_cli_login(sign_in: SignInId, code: Secret) -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.inner.cli_login.finish(sign_in, code).await,
        None => not_yet(),
    }
}

/// Answers `CheckCliLogin`.
pub fn check_cli_login(sign_in: SignInId) -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.inner.cli_login.check(sign_in),
        None => not_yet(),
    }
}

/// Answers `CancelCliLogin`.
pub fn cancel_cli_login(sign_in: SignInId) -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.inner.cli_login.cancel(sign_in),
        None => not_yet(),
    }
}

/// Answers `ForgetCredential`.
pub async fn forget(credential: CredentialId) -> ActionOutcome {
    match STORE.get() {
        Some(store) => store.forget(credential).await,
        None => not_yet(),
    }
}

/// An access token for Codex on `account`, from this device's ChatGPT
/// sign-in. The guest link asks on behalf of a Codex session `maxd`
/// started on that account.
pub async fn codex_token(account: AccountId) -> TokenReply {
    match STORE.get() {
        Some(store) => store.chatgpt_token(account).await,
        None => TokenReply::Refused {
            reason: "this device's secrets aren't open yet".to_owned(),
        },
    }
}

/// An installation token that reaches `repo` alone, with W-6's three
/// permissions, from this device's GitHub App. `repo` is "owner/name" or a
/// GitHub clone URL. The guest link asks on behalf of a session in a
/// workspace whose project has that repo, and the push preflight asks to
/// check the App reaches a project's repo.
pub async fn github(repo: &str) -> Result<GithubToken, GithubError> {
    match STORE.get() {
        Some(store) => store.github(repo).await,
        None => Err(GithubError::NoApp),
    }
}

/// Your GitHub user token on this device, which merges and approves as
/// you, or [`GithubError::NotSignedIn`].
pub async fn github_user() -> Result<UserToken, GithubError> {
    match STORE.get() {
        Some(store) => store.github_user().await,
        None => Err(GithubError::NotSignedIn),
    }
}

/// This device's pull requests on GitHub, read through its cache, once its
/// secrets are open.
pub fn pull_source() -> Option<&'static dyn PullSource> {
    STORE.get().map(|store| store as &dyn PullSource)
}

/// `GET path` on GitHub's API with this device's GitHub App's token for
/// `repo`, asking only for a change since `etag`, and how long GitHub asks
/// a poller to wait. The GitHub event pollers read with it.
pub async fn github_poll<T: DeserializeOwned>(
    repo: &RepoName,
    path: &str,
    etag: Option<&str>,
) -> Result<(Fetched<T>, Option<Duration>), GithubError> {
    match STORE.get() {
        Some(store) => store.github_poll(repo, path, etag).await,
        None => Err(GithubError::NoApp),
    }
}

/// Answers the composer's `#` lookup for `query` in `repo`, "owner/name"
/// or a GitHub clone URL, with this device's GitHub App or else its `gh`.
/// Why nothing could be looked up comes back in the reply's `unavailable`.
pub async fn find_git_items(repo: &str, query: &str) -> GitItemSearch {
    match STORE.get() {
        Some(store) => store.find_git_items(repo, query).await,
        None => GitItemSearch {
            repo: github::RepoName::parse(repo).map(|repo| repo.to_string()),
            items: Vec::new(),
            unavailable: Some(GithubError::NoApp.to_string()),
        },
    }
}

/// Whether `a` and `b` name the same GitHub repository, as "owner/name"
/// or clone URLs and whatever their case. The guest link checks this
/// between the repo a session asks for and its project's before it calls
/// [`github`].
pub fn same_repo(a: &str, b: &str) -> bool {
    match (github::RepoName::parse(a), github::RepoName::parse(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// A pasted token or key without the spaces and line breaks a terminal
/// wraps a long one in, or none when what's left is empty, longer than
/// `max` or not printable ASCII.
fn pasted(token: Secret, max: usize) -> Option<Secret> {
    let token = token.without_whitespace();
    let value = token.expose();
    let fits =
        !value.is_empty() && value.len() <= max && value.bytes().all(|b| b.is_ascii_graphic());
    fits.then_some(token)
}

/// Why a credential's name was refused.
const TOO_LONG: &str = "That name is too long.";

/// The name a credential gets from `label`, or `default` when it's blank.
/// None when it's longer than [`MAX_LABEL`].
fn named(label: &str, default: &str) -> Option<String> {
    let label = match label.trim() {
        "" => default,
        label => label,
    };
    (label.chars().count() <= MAX_LABEL).then(|| label.to_owned())
}

/// A refusal for a person to read.
fn refused(reason: impl Into<String>) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}
