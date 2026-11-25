// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Every path through the store, with known fake secrets and everything
//! logged down to TRACE kept. Then neither the log nor what the hub
//! publishes may hold any of those secrets.

use std::fs;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use maxhq_protocol::ids::AccountId;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::state::Entity;
use maxhq_protocol::Secret;

use super::chatgpt::fake::{Fake as ChatGptFake, Spoil};
use super::github::fake::{Fake, Install, APP_ID, DEVICE_CODE, TOKEN_PREFIX};
use super::testing::{
    app_key, pem, token_hub, Clock, CLAUDE_TOKEN, EC_PKCS8, RSA_PKCS1, RSA_SHA256,
};
use super::{claude_token_file, Credentials, Secrets, DIR};
use crate::session::clock::FakeClock;
use crate::testing::TempDir;

/// Everything written to the log, shared with the subscriber.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Log {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl io::Write for Log {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// What each path logs, so a log missing one wasn't the walk's whole log.
const SEEN: &[&str] = &[
    "stored a Claude setup-token",
    "skipped claude-",
    "forgot a Claude setup-token",
    "stored the GitHub App's key",
    RSA_SHA256,
    "checked the GitHub App",
    "the GitHub App's check failed",
    "minted a GitHub token",
    "loaded the GitHub App",
    "forgot the GitHub App",
    "made this device's ChatGPT host ID",
    "failed its check",
    "stored a ChatGPT sign-in",
    "renewed the ChatGPT session",
    "signed out of ChatGPT",
    "started a GitHub sign-in",
    "signed in to GitHub",
    "refreshed the GitHub sign-in",
    "signed out of GitHub",
    "stored the APNs key",
    "loaded the APNs key",
    "forgot the APNs key",
    "started a sign-in through the CLI",
    "a sign-in through the CLI ended",
];

#[tokio::test]
async fn no_secret_reaches_the_log_or_the_hub() {
    let mut secrets = vec![
        ("a setup-token", CLAUDE_TOKEN.to_owned()),
        ("a setup-token", "sk-ant-oat".to_owned()),
        ("a fake value", "test-not-a-real-token".to_owned()),
        ("an installation token", "ghs_".to_owned()),
        ("a JWT", "eyJ".to_owned()),
        ("a private key", "PRIVATE KEY".to_owned()),
        ("a ChatGPT access token", "test-access-".to_owned()),
        ("a ChatGPT refresh token", "test-refresh-".to_owned()),
        ("a sign-in code", "test-code-".to_owned()),
        ("a GitHub user token", "ghu_".to_owned()),
        ("a GitHub refresh token", "ghr_".to_owned()),
        ("a device code", DEVICE_CODE.to_owned()),
    ];
    let apns = super::apns::test_p8();
    for key in [RSA_PKCS1, apns.as_str()] {
        for chunk in key.as_bytes().as_chunks::<24>().0 {
            secrets.push(("a private key", String::from_utf8(chunk.to_vec()).unwrap()));
        }
    }

    // tracing caches which log lines anyone listens to. Another test
    // reaching a line for the first time can race this test's subscriber
    // and leave the line unheard, so a walk that missed a line runs again
    // once the cache is rebuilt.
    for _ in 0..5 {
        tracing::callsite::rebuild_interest_cache();
        let (log, published) = walk(&apns).await;
        for (what, secret) in &secrets {
            // Only what leaked is named, never the value or the log.
            assert!(!log.contains(secret.as_str()), "the log holds {what}");
            assert!(
                !published.contains(secret.as_str()),
                "the hub published {what}"
            );
        }
        if SEEN.iter().all(|line| log.contains(line)) {
            return;
        }
    }
    panic!("no walk logged every path");
}

/// Runs every path with a subscriber that keeps everything down to TRACE,
/// and returns the log and every entity the hubs published, as JSON.
async fn walk(apns: &str) -> (String, String) {
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);

    let fake = Fake::start().await;
    fake.state().installations = vec![Install {
        id: 7,
        owner: "octo".into(),
        repos: vec!["maxhq".into(), "Comet".into()],
        suspended: false,
        issues: Some("write"),
        reads: false,
        hooks: false,
    }];
    let temp = TempDir::new();
    let clock = Clock::at("2026-10-25T12:00:00Z");
    let openai = ChatGptFake::start(clock.now()).await;
    let open = || {
        Secrets::open_with(
            temp.path().join(DIR),
            token_hub(),
            fake.api(),
            fake.gh(),
            openai.issuer(),
            clock.now(),
        )
        .unwrap()
    };
    let secrets = open();
    let origin = AccountId::derive("mac-personal");

    // Claude: refused, stored and read back, then a broken file next to it.
    let bad = Secret::new(format!("{CLAUDE_TOKEN}é{CLAUDE_TOKEN}"));
    assert_ne!(
        secrets.store_claude_token(origin, String::new(), bad),
        ActionOutcome::Done
    );
    let stored =
        secrets.store_claude_token(origin, "Personal".into(), Secret::new(CLAUDE_TOKEN.into()));
    assert_eq!(stored, ActionOutcome::Done);
    let account = super::claude::token_account(secrets.device(), origin);
    assert_eq!(
        secrets.claude_token(account).unwrap().expose(),
        CLAUDE_TOKEN
    );
    let broken = AccountId::derive("mac-broken");
    fs::write(
        temp.path().join(DIR).join(claude_token_file(broken)),
        format!(r#"{{"token":"{CLAUDE_TOKEN}","stored_at":"{CLAUDE_TOKEN}"}}"#),
    )
    .unwrap();

    // APNs: a key refused, then one stored and a token made from it.
    let set = |key: &str| secrets.set_apns_key("ABC123DEFG", "TEAM123456", Secret::new(key.into()));
    assert_ne!(set(&pem("PRIVATE KEY", EC_PKCS8)), ActionOutcome::Done);
    assert_eq!(set(apns), ActionOutcome::Done);
    secrets.apns().unwrap().token(clock.now()()).unwrap();

    // GitHub: keys refused, the App set and checked, tokens minted, cached
    // and refused, and a key GitHub refuses.
    let set = |key: String| secrets.set_github_app(APP_ID, Secret::new(key));
    assert_ne!(set(CLAUDE_TOKEN.into()), ActionOutcome::Done);
    assert_ne!(set(pem("PRIVATE KEY", EC_PKCS8)), ActionOutcome::Done);
    assert_eq!(set(app_key()), ActionOutcome::Done);
    assert!(secrets.check_github().await);
    let token = secrets.github("octo/maxhq").await.unwrap();
    assert!(token.value.expose().starts_with(TOKEN_PREFIX));
    secrets
        .github("https://github.com/octo/maxhq")
        .await
        .unwrap();
    secrets.github("octo/Comet").await.unwrap();
    secrets.github("octo/elsewhere").await.unwrap_err();
    secrets.github("not a repo").await.unwrap_err();
    fake.state().app_id = 1;
    assert!(!secrets.check_github().await);
    secrets.github("acme/tools").await.unwrap_err();
    fake.state().app_id = APP_ID;
    clock.advance(Duration::from_secs(2 * 60 * 60));
    assert!(secrets.check_github().await);

    // Your own sign-in, through the App's device flow, then refreshed.
    let polling = Arc::new(FakeClock::new());
    secrets.inner.github.user.set_clock(polling.clone());
    let begun = secrets.begin_github_user(None).await;
    assert!(matches!(begun, ActionOutcome::DeviceCode { .. }));
    while secrets.github_user().await.is_err() {
        polling.advance(Duration::from_secs(5));
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    clock.advance(Duration::from_secs(8 * 60 * 60));
    secrets.github_user().await.unwrap();

    // ChatGPT: a sign-in whose identity token fails its check, one that
    // works, and a token renewed.
    let sign_in = |spoil| {
        openai.state().spoil = spoil;
        async {
            let ActionOutcome::SignIn { sign_in, url, .. } = secrets.begin_chatgpt().await else {
                panic!("no round started");
            };
            let callback = openai.authorize(&url);
            let code = Secret::new(callback.code);
            secrets
                .finish_chatgpt(sign_in, code, callback.state, callback.client_id)
                .await
        }
    };
    assert_ne!(sign_in(Spoil::Signature).await, ActionOutcome::Done);
    assert_eq!(sign_in(Spoil::Nothing).await, ActionOutcome::Done);
    let codex = super::chatgpt::chatgpt_account(secrets.device());
    clock.advance(Duration::from_secs(2 * 60 * 60));
    secrets.chatgpt_token(codex).await;
    assert_eq!(openai.state().refreshes, 1);

    // A login signed in again through a fake CLI, whose link and code are
    // both secrets.
    #[cfg(unix)]
    cli_sign_in(&secrets, &temp, origin).await;

    // All read back, then forgotten.
    let again = open();
    assert!(again.claude_token(account).is_some());
    again.github("octo/maxhq").await.unwrap();
    let mut entities = secrets.inner.hub.snapshot().entities;
    entities.extend(again.inner.hub.snapshot().entities);
    // Files are forgotten on this thread, which the subscriber covers.
    for entity in again.inner.hub.snapshot().entities {
        if let Entity::Credential(credential) = entity {
            let outcome = match again.forget_chatgpt(credential.id).await {
                Some(outcome) => outcome,
                None => again.forget_file(credential.id),
            };
            assert!(matches!(
                outcome,
                ActionOutcome::Done | ActionOutcome::SignedOut { revoked: true }
            ));
        }
    }

    (log.text(), serde_json::to_string(&entities).unwrap())
}

/// Signs in through a fake Claude CLI, an sh script, which prints a link
/// and reads the code.
#[cfg(unix)]
async fn cli_sign_in(secrets: &Secrets, temp: &TempDir, origin: AccountId) {
    use maxhq_protocol::message::CliLoginState;

    use super::cli_login::Flow;
    use crate::drive::{Framing, Launch};

    let cli = temp.path().join("claude");
    let script = "#!/bin/sh\necho 'visit: https://claude.example/test-code-link'\nread code\n[ \"$code\" = test-code-pasted ]\n";
    fs::write(&cli, script).unwrap();
    maxhq_os::fsmode::set_mode(&cli, 0o755).unwrap();
    let launch = Launch {
        program: cli,
        args: Vec::new(),
        env: Vec::new(),
        env_remove: Vec::new(),
        cwd: temp.path().to_owned(),
        framing: Framing::ClaudeStreamJson,
        secret_env: Vec::new(),
        run_as: None,
    };
    let rounds = &secrets.inner.cli_login;
    let ActionOutcome::CliLogin { sign_in, .. } = rounds.start(origin, &launch, Flow::Claude).await
    else {
        panic!("no sign-in began");
    };
    let code = Secret::new("test-code-pasted".into());
    let signed_in = ActionOutcome::CliLoginState {
        state: CliLoginState::SignedIn,
    };
    assert_eq!(rounds.finish(sign_in, code).await, signed_in);
}
