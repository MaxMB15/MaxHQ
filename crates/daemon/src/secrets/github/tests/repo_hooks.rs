// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! This device's repository webhooks against the fake GitHub.

use super::*;
use crate::github_events::repo_hooks::{KeptHook, RepoHooks};

const URL: &str = "https://devbox.example.ts.net/maxhq/github/device-1/token-1";

fn repo() -> RepoName {
    RepoName::parse("octo/maxhq").unwrap()
}

/// A rig whose App is set, on an account that granted managing webhooks.
async fn rig() -> Rig {
    let rig = Rig::new().await;
    assert_eq!(rig.set(), ActionOutcome::Done);
    rig.fake.state().installations[0].hooks = true;
    rig
}

/// The fake's webhooks on `octo/maxhq`.
fn hooks(rig: &Rig) -> Vec<Value> {
    rig.fake.state().hooks.clone()
}

/// Keeps this device's hook on `octo/maxhq` sending `events` to `url`,
/// and turns it on when it's off, as the keeper does.
async fn keep(secrets: &Secrets, known: Option<u64>, url: &str, events: &[&str]) -> KeptHook {
    let kept = secrets.keep(&repo(), known, url, events).await.unwrap();
    if !kept.active {
        secrets.activate(&repo(), kept.id).await.unwrap();
    }
    kept
}

#[tokio::test]
async fn makes_a_hook_with_a_secret_it_keeps_and_turns_it_on_after() {
    let rig = rig().await;
    let kept = rig
        .secrets
        .keep(&repo(), None, URL, &["pull_request", "push"])
        .await
        .unwrap();
    assert!(kept.changed);
    // It's made off, with its secret already kept.
    assert!(!kept.active);
    let [hook] = hooks(&rig).try_into().unwrap();
    assert_eq!(hook["active"], json!(false));
    assert!(rig.secrets.secret(kept.id).is_some());
    rig.secrets.activate(&repo(), kept.id).await.unwrap();

    let [hook] = hooks(&rig).try_into().unwrap();
    assert_eq!(hook["id"], json!(kept.id));
    assert_eq!(hook["name"], json!("web"));
    assert_eq!(hook["active"], json!(true));
    assert_eq!(hook["events"], json!(["pull_request", "push"]));
    assert_eq!(hook["config"]["url"], json!(URL));
    assert_eq!(hook["config"]["content_type"], json!("json"));
    assert_eq!(hook["config"]["insecure_ssl"], json!("0"));
    let secret = rig.secrets.secret(kept.id).unwrap();
    assert_eq!(hook["config"]["secret"], json!(secret.expose()));
    assert!(secret.expose().len() >= 40);
    let file = rig.folder().join(format!("github-hook-{}.secret", kept.id));
    assert_eq!(mode(&file), 0o600);

    // The token it used may manage webhooks and nothing else.
    let mints = rig.fake.repo_mints();
    let (_, asked) = mints.last().unwrap();
    assert_eq!(
        asked["permissions"],
        json!({ "repository_hooks": "write", "metadata": "read" })
    );
}

#[tokio::test]
async fn leaves_a_hook_that_matches_and_updates_one_that_doesnt() {
    let rig = rig().await;
    let made = keep(&rig.secrets, None, URL, &["push"]).await;
    let again = keep(&rig.secrets, Some(made.id), URL, &["push"]).await;
    assert_eq!(
        (again.id, again.changed, again.active),
        (made.id, false, true)
    );

    let events = ["issues", "pull_request", "push"];
    let moved = "https://hooks.example.com/maxhq/github/device-1/token-1";
    let updated = keep(&rig.secrets, Some(made.id), moved, &events).await;
    assert_eq!(
        (updated.id, updated.changed, updated.active),
        (made.id, true, true)
    );
    let [hook] = hooks(&rig).try_into().unwrap();
    assert_eq!(hook["events"], json!(events));
    assert_eq!(hook["config"]["url"], json!(moved));
    // The secret stays the one it keeps.
    let secret = rig.secrets.secret(made.id).unwrap();
    assert_eq!(hook["config"]["secret"], json!(secret.expose()));
}

#[tokio::test]
async fn adopts_its_own_hook_by_its_whole_url_after_a_restart_and_leaves_others_alone() {
    let rig = rig().await;
    let made = keep(&rig.secrets, None, URL, &["push"]).await;
    let other = |id: u64, url: &str| {
        json!({
            "id": id,
            "repo": "octo/maxhq",
            "name": "web",
            "active": true,
            "events": ["push"],
            "config": { "url": url, "content_type": "json", "secret": "theirs" },
        })
    };
    // Another hook at the device's path, but without its token, isn't its.
    let theirs = [
        other(900, "https://ci.example.com/hook"),
        other(901, "https://ci.example.com/maxhq/github/device-1"),
        other(902, "https://ci.example.com/maxhq/github/device-1/token-2"),
    ];
    rig.fake.state().hooks.splice(0..0, theirs.clone());

    // The next start doesn't know the hook's ID, but finds it by its URL
    // and keeps the secret it already holds.
    let reopened = rig.reopen();
    let adopted = keep(&reopened, None, URL, &["push"]).await;
    assert_eq!(
        (adopted.id, adopted.changed, adopted.active),
        (made.id, false, true)
    );

    // Without its secret, or with one too short to sign with, it sets a
    // new one and leaves the hook off until the keeper turns it on.
    let file = rig.folder().join(format!("github-hook-{}.secret", made.id));
    for kept in [None, Some(""), Some("short")] {
        match kept {
            None => fs::remove_file(&file).unwrap(),
            Some(text) => fs::write(&file, text).unwrap(),
        }
        assert!(reopened.secret(made.id).is_none());
        let renewed = reopened.keep(&repo(), None, URL, &["push"]).await.unwrap();
        assert_eq!(
            (renewed.id, renewed.changed, renewed.active),
            (made.id, true, false)
        );
        let secret = reopened.secret(made.id).unwrap();
        let hooks = hooks(&rig);
        assert_eq!(hooks[3]["active"], json!(false));
        assert_eq!(hooks[3]["config"]["secret"], json!(secret.expose()));
        assert_eq!(hooks[..3], theirs);
        reopened.activate(&repo(), made.id).await.unwrap();
    }
}

#[tokio::test]
async fn keeps_one_doorbell_token_and_replaces_one_too_short() {
    let rig = rig().await;
    let token = rig.secrets.doorbell_token().unwrap();
    assert!(token.expose().len() >= 40);
    let file = rig.folder().join("github-doorbell.token");
    assert_eq!(mode(&file), 0o600);
    assert_eq!(
        rig.reopen().doorbell_token().unwrap().expose(),
        token.expose()
    );

    fs::write(&file, "").unwrap();
    let renewed = rig.reopen().doorbell_token().unwrap();
    assert!(renewed.expose().len() >= 40);
    assert_ne!(renewed.expose(), token.expose());
    assert_eq!(fs::read_to_string(&file).unwrap(), renewed.expose());
}

#[tokio::test]
async fn pings_and_deletes_its_hook_and_forgets_the_secret() {
    let rig = rig().await;
    let made = keep(&rig.secrets, None, URL, &["push"]).await;
    rig.secrets.ping(&repo(), made.id).await.unwrap();
    assert_eq!(rig.fake.state().pings, [made.id]);

    rig.secrets.remove(&repo(), made.id).await.unwrap();
    assert!(hooks(&rig).is_empty());
    assert!(rig.secrets.secret(made.id).is_none());
    // One already gone counts as deleted.
    rig.secrets.remove(&repo(), made.id).await.unwrap();
}

#[tokio::test]
async fn a_404_with_a_kept_token_is_asked_again_with_a_new_one() {
    let rig = rig().await;
    let made = keep(&rig.secrets, None, URL, &["push"]).await;
    let mints = || rig.fake.repo_mints().len();
    let minted = mints();

    // The token kept for the hooks no longer reaches the repository, so
    // GitHub answers 404. A new one finds the hook as it is.
    rig.fake.state().narrow_repo_tokens();
    let again = keep(&rig.secrets, Some(made.id), URL, &["push"]).await;
    assert_eq!((again.id, again.changed), (made.id, false));
    assert_eq!(mints(), minted + 1);

    // A delete deletes the hook, not just its secret.
    rig.fake.state().narrow_repo_tokens();
    rig.secrets.remove(&repo(), made.id).await.unwrap();
    assert!(hooks(&rig).is_empty());
    assert!(rig.secrets.secret(made.id).is_none());
    assert_eq!(mints(), minted + 2);

    // A hook already gone is a 404 to a new token too, which counts, and
    // is asked no more.
    rig.secrets.remove(&repo(), made.id).await.unwrap();
    assert_eq!(mints(), minted + 3);
    let deletes = rig
        .fake
        .state()
        .requests
        .iter()
        .filter(|request| request.method == "DELETE")
        .count();
    assert_eq!(deletes, 4);
}

#[tokio::test]
async fn a_hook_out_of_the_apps_reach_keeps_its_secret() {
    let rig = rig().await;
    let made = keep(&rig.secrets, None, URL, &["push"]).await;
    rig.fake.state().installations.clear();
    let reopened = rig.reopen();
    let err = reopened.remove(&repo(), made.id).await.unwrap_err();
    assert!(matches!(err, GithubError::NotInstalled(_)), "{err:?}");
    // GitHub may still send from it, so it can still be checked.
    assert!(reopened.secret(made.id).is_some());
    assert_eq!(hooks(&rig).len(), 1);
}

#[tokio::test]
async fn an_account_that_didnt_grant_webhooks_is_refused() {
    let rig = rig().await;
    rig.fake.state().installations[0].hooks = false;
    let err = rig
        .secrets
        .keep(&repo(), None, URL, &["push"])
        .await
        .unwrap_err();
    assert!(matches!(err, GithubError::HooksNotGranted(_)), "{err:?}");
    assert!(hooks(&rig).is_empty());
}

#[tokio::test]
async fn names_the_apps_permissions_page() {
    let rig = rig().await;
    assert_eq!(rig.secrets.permissions_page(), None);
    assert!(rig.secrets.check_github().await);
    assert_eq!(
        rig.secrets.permissions_page().as_deref(),
        Some(format!("https://github.com/settings/apps/{SLUG}/permissions").as_str())
    );
    rig.fake.state().app_org = Some("acme");
    assert!(rig.secrets.check_github().await);
    assert_eq!(
        rig.secrets.permissions_page().as_deref(),
        Some(
            format!("https://github.com/organizations/acme/settings/apps/{SLUG}/permissions")
                .as_str()
        )
    );
}
