// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The App against a fake GitHub on 127.0.0.1.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::fake::{Fake, Install, APP_ID, SLUG, TOKEN_PREFIX};
use super::*;
use crate::secrets::chatgpt::Issuer;
use crate::secrets::testing::{app_key, at, hub, pem, Clock, EC_PKCS8, RSA_SHA256, UNUSED_ISSUER};
use crate::secrets::DIR;
use crate::testing::TempDir;

mod items;
mod pulls;
mod repo_hooks;
mod user;

/// A store with a fake GitHub where the App is on three accounts, one of
/// which suspended it.
struct Rig {
    temp: TempDir,
    fake: Fake,
    clock: Clock,
    secrets: Secrets,
}

impl Rig {
    async fn new() -> Self {
        let fake = Fake::start().await;
        fake.state().installations = vec![
            install(7, "octo", &["maxhq", "Comet"], false),
            install(9, "acme", &["tools"], false),
            install(11, "paused", &["old"], true),
        ];
        let temp = TempDir::new();
        let clock = Clock::at("2026-10-25T12:00:00Z");
        let secrets = open(&temp, &fake, &clock);
        Self {
            temp,
            fake,
            clock,
            secrets,
        }
    }

    /// The store as the next start of `maxd` opens it.
    fn reopen(&self) -> Secrets {
        open(&self.temp, &self.fake, &self.clock)
    }

    fn folder(&self) -> PathBuf {
        self.temp.path().join(DIR)
    }

    fn set(&self) -> ActionOutcome {
        self.secrets.set_github_app(APP_ID, Secret::new(app_key()))
    }

    fn credential(&self) -> Option<Credential> {
        credential(&self.secrets)
    }

    fn file(&self) -> Value {
        let text = fs::read_to_string(self.folder().join(GITHUB_APP_FILE)).unwrap();
        serde_json::from_str(&text).unwrap()
    }
}

fn open(temp: &TempDir, fake: &Fake, clock: &Clock) -> Secrets {
    Secrets::open_with(
        temp.path().join(DIR),
        hub(),
        fake.api(),
        fake.gh(),
        Issuer::local(UNUSED_ISSUER),
        clock.now(),
    )
    .unwrap()
}

fn install(id: u64, owner: &str, repos: &[&str], suspended: bool) -> Install {
    Install {
        id,
        owner: owner.into(),
        repos: repos.iter().map(|&repo| repo.into()).collect(),
        suspended,
        issues: Some("write"),
        reads: false,
        hooks: false,
    }
}

fn credential(secrets: &Secrets) -> Option<Credential> {
    let key = EntityKey::Credential(github_credential(secrets.device()));
    match secrets.inner.hub.get(&key) {
        Some(Entity::Credential(credential)) => Some(credential),
        _ => None,
    }
}

/// The App's slug and the repositories it reaches, as published.
fn reach(credential: &Credential) -> (Option<&str>, Vec<&str>) {
    let CredentialKind::GithubApp { slug, repos, .. } = &credential.kind else {
        panic!("not the App");
    };
    (slug.as_deref(), repos.iter().map(String::as_str).collect())
}

fn mode(path: &Path) -> u32 {
    maxhq_os::fsmode::mode_of(&fs::metadata(path).unwrap()) & 0o777
}

/// Waits up to five seconds for `probe` to find something.
async fn eventually<T>(mut probe: impl FnMut() -> Option<T>) -> T {
    for _ in 0..500 {
        if let Some(found) = probe() {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("it never happened");
}

#[tokio::test]
async fn stores_the_key_privately() {
    let rig = Rig::new().await;
    assert_eq!(rig.set(), ActionOutcome::Done);

    let folder = rig.folder();
    assert_eq!(mode(&folder), 0o700);
    assert_eq!(mode(&folder.join(GITHUB_KEY_FILE)), 0o600);
    assert_eq!(mode(&folder.join(GITHUB_APP_FILE)), 0o600);
    assert_eq!(
        fs::read_to_string(folder.join(GITHUB_KEY_FILE)).unwrap(),
        app_key()
    );
    let file = rig.file();
    assert_eq!(file["app_id"], APP_ID);
    assert_eq!(file["key_sha256"], RSA_SHA256);

    let credential = rig.credential().unwrap();
    assert_eq!(
        credential.kind,
        CredentialKind::GithubApp {
            app_id: APP_ID,
            slug: None,
            key_sha256: RSA_SHA256.into(),
            repos: Vec::new(),
        }
    );
    assert_eq!(credential.stored_at, at("2026-10-25T12:00:00Z"));
    assert_eq!(credential.problem, None);
    // Setting it alone asks GitHub nothing. The check does that.
    assert!(rig.fake.state().requests.is_empty());
}

#[tokio::test]
async fn refuses_an_app_it_cant_use() {
    let rig = Rig::new().await;
    let refusal = |app_id, key: String| match rig.secrets.set_github_app(app_id, Secret::new(key)) {
        ActionOutcome::Refused { reason } => reason,
        other => panic!("expected a refusal, got {other:?}"),
    };
    assert!(refusal(0, app_key()).contains("App ID"));
    assert!(refusal(APP_ID, pem("PRIVATE KEY", EC_PKCS8)).contains("isn't an RSA key"));
    assert!(refusal(APP_ID, "test-not-a-real-token".into()).contains("isn't a private key"));
    assert!(refusal(APP_ID, pem("ENCRYPTED PRIVATE KEY", "AAAA")).contains("passphrase"));

    assert!(rig.credential().is_none());
    assert!(fs::read_dir(rig.folder()).unwrap().next().is_none());
}

#[tokio::test]
async fn the_check_finds_the_app_and_what_it_reaches() {
    let rig = Rig::new().await;
    rig.set();
    assert!(rig.secrets.check_github().await);

    let credential = rig.credential().unwrap();
    assert_eq!(
        reach(&credential),
        (Some(SLUG), vec!["acme/tools", "octo/Comet", "octo/maxhq"])
    );
    assert_eq!(credential.problem, None);
    let file = rig.file();
    assert_eq!(file["slug"], SLUG);
    assert_eq!(
        file["repos"],
        json!(["acme/tools", "octo/Comet", "octo/maxhq"])
    );

    // It minted for the first repository, which proves the key works.
    let mints = rig.fake.repo_mints();
    assert_eq!(mints.len(), 1);
    assert_eq!(mints[0].0, "/app/installations/9/access_tokens");
    // The suspended installation got no token.
    let state = rig.fake.state();
    assert!(!state
        .requests
        .iter()
        .any(|request| request.path.contains("/11/")));
}

#[tokio::test]
async fn the_check_says_when_the_app_reaches_nothing() {
    let rig = Rig::new().await;
    rig.fake.state().installations.clear();
    rig.set();
    assert!(!rig.secrets.check_github().await);

    let credential = rig.credential().unwrap();
    assert_eq!(reach(&credential), (Some(SLUG), Vec::new()));
    assert!(credential
        .problem
        .unwrap()
        .contains("isn't installed on any repositories"));
}

#[tokio::test]
async fn mints_for_one_repo_with_its_permissions() {
    let rig = Rig::new().await;
    rig.set();
    let token = rig
        .secrets
        .github("https://github.com/octo/maxhq.git")
        .await
        .unwrap();
    assert!(token.value.expose().starts_with(TOKEN_PREFIX));
    assert_eq!(token.expires_at, at("2026-10-25T13:00:00Z"));

    // The installation reaches two repositories, and the token only one.
    assert_eq!(
        rig.fake.repo_mints(),
        [(
            "/app/installations/7/access_tokens".to_owned(),
            json!({
                "repositories": ["maxhq"],
                "permissions": {
                    "contents": "write",
                    "pull_requests": "write",
                    "metadata": "read",
                    "issues": "write",
                },
            })
        )]
    );
    let state = rig.fake.state();
    assert!(state
        .requests
        .iter()
        .any(|request| request.path == "/repos/octo/maxhq/installation"));
}

/// What the last token asked for on issues.
fn issues_asked(fake: &Fake) -> Value {
    let mints = fake.repo_mints();
    mints.last().unwrap().1["permissions"]["issues"].clone()
}

#[tokio::test]
async fn asks_for_issues_only_as_far_as_the_account_granted_them() {
    let rig = Rig::new().await;
    rig.fake.state().installations[0].issues = None;
    rig.fake.state().installations[1].issues = Some("read");
    rig.set();

    // Before the check, from the repository's own installation.
    rig.secrets.github("octo/maxhq").await.unwrap();
    assert_eq!(issues_asked(&rig.fake), Value::Null);
    rig.secrets.github("acme/tools").await.unwrap();
    assert_eq!(issues_asked(&rig.fake), json!("read"));

    // The check mints for acme/tools, which works, and still names the
    // account that hasn't granted issues.
    assert!(!rig.secrets.check_github().await);
    let problem = rig.credential().unwrap().problem.unwrap();
    assert!(
        problem.contains("can't reach issues on octo until"),
        "{problem}"
    );
    rig.secrets.github("octo/Comet").await.unwrap();
    assert_eq!(issues_asked(&rig.fake), Value::Null);
}

#[tokio::test]
async fn the_check_says_when_the_app_doesnt_ask_for_issues() {
    let rig = Rig::new().await;
    rig.fake.state().app_issues = None;
    for install in &mut rig.fake.state().installations {
        install.issues = None;
    }
    rig.set();
    assert!(!rig.secrets.check_github().await);

    let credential = rig.credential().unwrap();
    assert_eq!(
        reach(&credential),
        (Some(SLUG), vec!["acme/tools", "octo/Comet", "octo/maxhq"])
    );
    let problem = credential.problem.unwrap();
    assert!(problem.contains("Permissions & events"), "{problem}");
}

#[tokio::test]
async fn a_token_gets_issues_once_the_account_accepts_them() {
    let rig = Rig::new().await;
    rig.fake.state().installations[0].issues = None;
    rig.set();
    assert!(!rig.secrets.check_github().await);
    let before = rig.secrets.github("octo/maxhq").await.unwrap();
    assert_eq!(issues_asked(&rig.fake), Value::Null);

    // The account accepts, and checking again mints a token with issues
    // instead of reusing the one without.
    rig.fake.state().installations[0].issues = Some("write");
    assert!(rig.secrets.check_github().await);
    assert_eq!(rig.credential().unwrap().problem, None);
    let after = rig.secrets.github("octo/maxhq").await.unwrap();
    assert_ne!(after.value.expose(), before.value.expose());
    assert_eq!(issues_asked(&rig.fake), json!("write"));
}

#[tokio::test]
async fn reuses_a_token_until_five_minutes_before_it_expires() {
    let rig = Rig::new().await;
    rig.set();
    let first = rig.secrets.github("octo/maxhq").await.unwrap();

    let expires = at("2026-10-25T13:00:00Z");
    rig.clock.set(Timestamp(expires.0 - 5 * 60 * 1_000 - 1));
    let again = rig.secrets.github("OCTO/MaxHQ").await.unwrap();
    assert_eq!(again.value.expose(), first.value.expose());
    assert_eq!(rig.fake.repo_mints().len(), 1);

    rig.fake.state().expires_at = "2026-10-25T14:00:00Z".into();
    rig.clock.advance(Duration::from_millis(1));
    let fresh = rig.secrets.github("octo/maxhq").await.unwrap();
    assert_ne!(fresh.value.expose(), first.value.expose());
    assert_eq!(fresh.expires_at, at("2026-10-25T14:00:00Z"));
    assert_eq!(rig.fake.repo_mints().len(), 2);
}

#[tokio::test]
async fn polls_a_repo_with_its_token_and_githubs_interval() {
    let rig = Rig::new().await;
    rig.set();
    {
        let mut state = rig.fake.state();
        state.events = vec![json!({ "id": "1", "type": "PushEvent" })];
        state.poll_interval = Some(90);
    }
    let repo = RepoName::parse("octo/maxhq").unwrap();
    let path = "/repos/octo/maxhq/events?per_page=100";
    let (fetched, poll_after) = rig
        .secrets
        .github_poll::<Value>(&repo, path, None)
        .await
        .unwrap();
    assert_eq!(poll_after, Some(Duration::from_secs(90)));
    let Fetched::Fresh { value, etag } = fetched else {
        panic!("the first poll is fresh");
    };
    assert_eq!(value[0]["id"], "1");

    let (again, poll_after) = rig
        .secrets
        .github_poll::<Value>(&repo, path, etag.as_deref())
        .await
        .unwrap();
    assert!(matches!(again, Fetched::Unchanged));
    assert_eq!(poll_after, Some(Duration::from_secs(90)));

    rig.fake.state().poll_interval = None;
    let (_, poll_after) = rig
        .secrets
        .github_poll::<Value>(&repo, path, None)
        .await
        .unwrap();
    assert_eq!(poll_after, None);
    assert_eq!(rig.fake.repo_mints().len(), 1);

    let elsewhere = RepoName::parse("octo/elsewhere").unwrap();
    let refused = rig
        .secrets
        .github_poll::<Value>(&elsewhere, "/repos/octo/elsewhere/events", None)
        .await;
    assert_eq!(
        refused.err(),
        Some(GithubError::NotInstalled("octo/elsewhere".into()))
    );
}

#[tokio::test]
async fn refuses_a_repo_the_app_doesnt_reach() {
    let rig = Rig::new().await;
    rig.set();
    let secrets = &rig.secrets;
    assert_eq!(
        secrets.github("octo/elsewhere").await.unwrap_err(),
        GithubError::NotInstalled("octo/elsewhere".into())
    );
    assert_eq!(
        secrets.github("paused/old").await.unwrap_err(),
        GithubError::NotInstalled("paused/old".into())
    );
    for repo in [
        "https://gitlab.com/octo/maxhq",
        "https://someone@github.com/octo/maxhq",
        "octo/maxhq/extra",
        "",
    ] {
        assert_eq!(
            secrets.github(repo).await.unwrap_err(),
            GithubError::BadRepo,
            "{repo}"
        );
    }
    assert!(rig.fake.repo_mints().is_empty());
}

#[tokio::test]
async fn shows_a_refused_key_until_github_takes_it_again() {
    let rig = Rig::new().await;
    rig.set();
    // GitHub now has another App ID on file, so it refuses every JWT.
    rig.fake.state().app_id = 1;
    assert!(!rig.secrets.check_github().await);
    let problem = rig.credential().unwrap().problem.unwrap();
    assert!(
        problem.contains("A JSON web token could not be decoded"),
        "{problem}"
    );

    assert!(matches!(
        rig.secrets.github("octo/maxhq").await,
        Err(GithubError::Refused { status: 401, .. })
    ));
    let problem = rig.credential().unwrap().problem.unwrap();
    assert!(
        problem.starts_with("GitHub refused the App's key"),
        "{problem}"
    );

    rig.fake.state().app_id = APP_ID;
    rig.secrets.github("octo/maxhq").await.unwrap();
    assert_eq!(rig.credential().unwrap().problem, None);
}

#[tokio::test]
async fn opening_again_keeps_the_app() {
    let rig = Rig::new().await;
    rig.set();
    assert!(rig.secrets.check_github().await);

    let again = rig.reopen();
    let credential = credential(&again).unwrap();
    assert_eq!(
        reach(&credential),
        (Some(SLUG), vec!["acme/tools", "octo/Comet", "octo/maxhq"])
    );
    assert_eq!(credential.stored_at, at("2026-10-25T12:00:00Z"));
    // The key it read back still signs.
    again.github("acme/tools").await.unwrap();
}

#[tokio::test]
async fn forgetting_deletes_the_key_and_its_file() {
    let rig = Rig::new().await;
    rig.set();
    rig.secrets.github("octo/maxhq").await.unwrap();
    let id = rig.credential().unwrap().id;

    assert_eq!(rig.secrets.forget(id).await, ActionOutcome::Done);
    assert!(!rig.folder().join(GITHUB_KEY_FILE).exists());
    assert!(!rig.folder().join(GITHUB_APP_FILE).exists());
    assert!(rig.credential().is_none());
    // The cached token goes with it.
    assert_eq!(
        rig.secrets.github("octo/maxhq").await.unwrap_err(),
        GithubError::NoApp
    );
    assert!(credential(&rig.reopen()).is_none());
    assert!(matches!(
        rig.secrets.forget(id).await,
        ActionOutcome::Refused { .. }
    ));
}

#[tokio::test]
async fn forgetting_revokes_every_kept_token() {
    let rig = Rig::new().await;
    rig.set();
    let one = rig.secrets.github("octo/maxhq").await.unwrap();
    let two = rig.secrets.github("acme/tools").await.unwrap();
    let id = rig.credential().unwrap().id;

    assert_eq!(rig.secrets.forget(id).await, ActionOutcome::Done);

    let revoked = eventually(|| {
        let tokens: Vec<String> = rig
            .fake
            .state()
            .requests
            .iter()
            .filter(|request| request.method == "DELETE" && request.path == "/installation/token")
            .filter_map(|request| request.auth.clone())
            .collect();
        (tokens.len() == 2).then_some(tokens)
    })
    .await;
    assert!(revoked.contains(&one.value.expose().to_owned()));
    assert!(revoked.contains(&two.value.expose().to_owned()));
}

#[tokio::test]
async fn forgetting_works_when_github_is_out_of_reach() {
    let rig = Rig::new().await;
    rig.set();
    rig.secrets.github("octo/maxhq").await.unwrap();
    let id = rig.credential().unwrap().id;
    rig.fake.state().offline = true;

    assert_eq!(rig.secrets.forget(id).await, ActionOutcome::Done);

    assert!(!rig.folder().join(GITHUB_KEY_FILE).exists());
    assert!(!rig.folder().join(GITHUB_APP_FILE).exists());
    assert!(rig.credential().is_none());
}

#[tokio::test]
async fn opening_drops_half_an_app_and_skips_a_key_that_doesnt_match() {
    let rig = Rig::new().await;
    rig.set();
    let folder = rig.folder();
    fs::remove_file(folder.join(GITHUB_APP_FILE)).unwrap();
    assert!(credential(&rig.reopen()).is_none());
    assert!(!folder.join(GITHUB_KEY_FILE).exists());

    rig.set();
    let mut file = rig.file();
    file["key_sha256"] = json!("00".repeat(32));
    fs::write(folder.join(GITHUB_APP_FILE), file.to_string()).unwrap();
    assert!(credential(&rig.reopen()).is_none());
}

#[tokio::test]
async fn a_new_app_drops_the_old_ones_tokens() {
    let rig = Rig::new().await;
    rig.set();
    let first = rig.secrets.github("octo/maxhq").await.unwrap();
    assert_eq!(rig.set(), ActionOutcome::Done);
    let second = rig.secrets.github("octo/maxhq").await.unwrap();
    assert_ne!(first.value.expose(), second.value.expose());
    assert_eq!(rig.fake.repo_mints().len(), 2);
}

#[tokio::test]
async fn callers_asking_at_once_share_one_token() {
    let rig = Rig::new().await;
    rig.set();
    let secrets = &rig.secrets;
    let (a, b, c) = tokio::join!(
        secrets.github("octo/maxhq"),
        secrets.github("octo/maxhq"),
        secrets.github("git@github.com:octo/maxhq.git"),
    );
    let (a, b, c) = (a.unwrap(), b.unwrap(), c.unwrap());
    assert_eq!(a.value.expose(), b.value.expose());
    assert_eq!(a.value.expose(), c.value.expose());
    assert_eq!(rig.fake.repo_mints().len(), 1);
}

#[tokio::test]
async fn the_background_check_runs_when_the_app_is_set() {
    let rig = Rig::new().await;
    let running = tokio::spawn(rig.secrets.clone().run());
    rig.set();
    let repos = eventually(|| {
        let credential = rig.credential()?;
        let (slug, repos) = reach(&credential);
        slug.is_some().then_some(repos.len())
    })
    .await;
    running.abort();
    assert_eq!(repos, 3);
}

#[tokio::test]
async fn checking_again_wakes_the_background_check() {
    let rig = Rig::new().await;
    let installations = std::mem::take(&mut rig.fake.state().installations);
    rig.set();
    // Setting the App left a wake for the check. Take it, so the one below
    // is the request's alone.
    rig.secrets.inner.github.wake.notified().await;
    let running = tokio::spawn(rig.secrets.clone().run());
    eventually(|| {
        let problem = rig.credential()?.problem?;
        problem
            .contains("isn't installed on any repositories")
            .then_some(())
    })
    .await;

    // Installed now, with the next check five minutes off: only the
    // request finds it in time.
    rig.fake.state().installations = installations;
    assert_eq!(rig.secrets.check_github_app(), ActionOutcome::Done);
    let repos = eventually(|| {
        let credential = rig.credential()?;
        let (_, repos) = reach(&credential);
        credential.problem.is_none().then_some(repos.len())
    })
    .await;
    running.abort();
    assert_eq!(repos, 3);
}

#[tokio::test]
async fn checking_again_with_no_app_says_so() {
    let rig = Rig::new().await;
    let refused = |outcome| match outcome {
        ActionOutcome::Refused { reason } => reason,
        other => panic!("expected a refusal, got {other:?}"),
    };
    assert_eq!(
        refused(rig.secrets.check_github_app()),
        "This device doesn't hold a GitHub App, so there's nothing to check."
    );

    rig.set();
    let id = rig.credential().unwrap().id;
    assert_eq!(rig.secrets.forget(id).await, ActionOutcome::Done);
    assert!(refused(rig.secrets.check_github_app()).contains("doesn't hold a GitHub App"));
}

#[test]
fn reads_repo_names_and_clone_urls() {
    let want = RepoName::parse("octo/maxhq").unwrap();
    assert_eq!(want.owner(), "octo");
    assert_eq!(want.name(), "maxhq");
    assert_eq!(want.to_string(), "octo/maxhq");
    for text in [
        "OCTO/MaxHQ",
        " octo/maxhq\n",
        "octo/maxhq.git",
        "https://github.com/octo/maxhq",
        "https://github.com/octo/maxhq.git",
        "https://github.com/octo/maxhq/",
        "git@github.com:octo/maxhq.git",
        "ssh://git@github.com/octo/maxhq.git",
    ] {
        assert_eq!(RepoName::parse(text).as_ref(), Some(&want), "{text}");
    }
    assert!(RepoName::parse("octo/max.hq_2-x").is_some());

    let long = format!("{}/maxhq", "a".repeat(40));
    for text in [
        "",
        "octo",
        "octo/",
        "/maxhq",
        "octo/maxhq/extra",
        "octo/..",
        "oc to/maxhq",
        "octo/max hq",
        "http://github.com/octo/maxhq",
        "https://gitlab.com/octo/maxhq",
        "https://someone@github.com/octo/maxhq",
        long.as_str(),
    ] {
        assert_eq!(RepoName::parse(text), None, "{text}");
    }
}

#[test]
fn turns_into_the_guest_links_reply() {
    let token = GithubToken {
        value: Secret::new(format!("{TOKEN_PREFIX}1")),
        expires_at: at("2026-10-25T13:00:00Z"),
    };
    let TokenReply::Token { value, expires_at } = TokenReply::from(token) else {
        panic!("not a token");
    };
    assert!(value.expose().starts_with(TOKEN_PREFIX));
    assert_eq!(expires_at, at("2026-10-25T13:00:00Z"));

    let TokenReply::Refused { reason } =
        TokenReply::from(GithubError::NotInstalled("octo/x".into()))
    else {
        panic!("not a refusal");
    };
    assert_eq!(
        reason,
        "The GitHub App isn't installed on octo/x. Install it there from the App's page on GitHub."
    );
}

#[tokio::test]
async fn no_one_is_signed_in_to_github_yet() {
    let rig = Rig::new().await;
    let err = rig.secrets.github_user().await.expect_err("no sign-in");
    assert_eq!(err, GithubError::NotSignedIn);
    assert_eq!(
        err.to_string(),
        "Sign in to GitHub in Settings to merge and approve here."
    );
}
