// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The device's own Cursor login: the account the CLI signed in with
//! `agent login`, read from `agent status` and `agent about`. A CLI that's
//! signed out keeps its account, logged out, so you can sign it in again
//! from MaxHQ. Once you turn Cursor usage on for a Mac, the plan's meters
//! for the month come from Cursor's dashboard as well, asked with the
//! CLI's login from the Keychain. Nothing here reads the CLI's files, and
//! nothing logs the login or what Cursor sends.

mod keychain;
mod period;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use maxhq_core::names;
use maxhq_protocol::ids::{AccountId, DeviceId};
use maxhq_protocol::model::{Account, LoginKind, Usage, UsageFailure, Vendor};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use serde_json::Value;
use tokio::process::Command as Process;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::MissedTickBehavior;
use tracing::debug;
use zeroize::Zeroizing;

use self::keychain::Keychain;
use self::period::Failure;
use crate::drive::cursor;
use crate::hub::{Command, Hub};
use crate::secrets::http;

/// How often the login and the meters are read.
pub const CHECK_EVERY: Duration = Duration::from_secs(5 * 60);

/// How long one CLI call may take.
const TIMEOUT: Duration = Duration::from_secs(20);

/// What the app shows unless you rename the account.
const LABEL: &str = "Cursor";

/// Why a signed-out CLI's account has no usage, as a Claude Code login's
/// check says it.
const LOGGED_OUT: &str = "logged out";

/// The most checks on the timer a run of failures skips, so a check that
/// keeps failing still runs every hour.
const MOST_SKIPPED: u32 = 11;

/// Who the CLI is signed in as, or nothing while it's signed out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Login {
    email: Option<String>,
    plan: Option<String>,
}

/// Where the meters come from, so tests stand in for the Keychain and
/// Cursor.
trait Source: Send + 'static {
    /// The CLI's login, or none when it saved none.
    fn token(&mut self) -> impl Future<Output = keychain::Read> + Send;
    /// What Cursor says about the current period.
    fn period(&mut self, token: &str) -> impl Future<Output = Result<Value, Failure>> + Send;
}

/// The Keychain and Cursor themselves.
#[derive(Default)]
struct Live {
    keychain: Keychain,
    client: Option<reqwest::Client>,
}

impl Source for Live {
    async fn token(&mut self) -> keychain::Read {
        self.keychain.token().await
    }

    async fn period(&mut self, token: &str) -> Result<Value, Failure> {
        let client = match &self.client {
            Some(client) => client,
            None => self.client.insert(
                http::client(true)
                    .map_err(|_| Failure::Other("couldn't set up a connection".into()))?,
            ),
        };
        period::fetch(client, period::ENDPOINT, token).await
    }
}

/// The month's meters as last read, and what reading them again needs.
struct Meters<S> {
    source: S,
    /// The login last read, kept until Cursor turns it down or the CLI
    /// signs in as someone else.
    token: Option<Zeroizing<String>>,
    /// Whose login that is.
    email: Option<String>,
    /// What the last check gave.
    last: Option<Result<Usage, UsageFailure>>,
    /// Failed checks in a row.
    failures: u32,
    /// Checks on the timer to skip before trying again.
    skip: u32,
}

impl<S: Source> Meters<S> {
    fn new(source: S) -> Self {
        Self {
            source,
            token: None,
            email: None,
            last: None,
            failures: 0,
            skip: 0,
        }
    }

    /// Drops what was read, as when you turn usage off or sign out.
    fn forget(&mut self) {
        self.token = None;
        self.email = None;
        self.last = None;
        self.failures = 0;
        self.skip = 0;
    }

    /// The meters for `login`. A check on the timer waits out a run of
    /// failures, and one you ask for runs now.
    async fn read(&mut self, login: &Login, asked: bool) -> Result<Usage, UsageFailure> {
        if self.email != login.email {
            self.forget();
            self.email.clone_from(&login.email);
        }
        if let Some(last) = &self.last
            && self.skip > 0
            && !asked
        {
            self.skip -= 1;
            return last.clone();
        }
        let at = Timestamp::now();
        let result = self.check(at).await.map_err(|reason| {
            debug!(%reason, "couldn't read Cursor's usage");
            UsageFailure { at, reason }
        });
        if result.is_ok() {
            self.failures = 0;
            self.skip = 0;
        } else {
            self.failures += 1;
            self.skip = ((1 << self.failures.min(4)) - 1).min(MOST_SKIPPED);
        }
        self.last = Some(result.clone());
        result
    }

    /// Asks Cursor, reading the login first when none is kept.
    async fn check(&mut self, at: Timestamp) -> Result<Usage, String> {
        let token = match self.token.take() {
            Some(token) => token,
            None => self
                .source
                .token()
                .await?
                .ok_or("the Cursor CLI has no login in the Keychain")?,
        };
        match self.source.period(&token).await {
            Ok(body) => {
                self.token = Some(token);
                period::usage(&body, at).ok_or_else(|| "Cursor sent no meters for this plan".into())
            }
            // The CLI saves a fresh login when it next runs, so the next
            // check reads the Keychain again.
            Err(Failure::Rejected) => Err("Cursor turned down the saved login".into()),
            Err(Failure::Other(reason)) => {
                self.token = Some(token);
                Err(reason)
            }
        }
    }
}

/// Keeps the device's Cursor account current until the process exits. A
/// device with no Cursor CLI has none.
pub async fn run(hub: Hub) {
    let (Some(program), Some(home)) = (cursor::program(), std::env::home_dir()) else {
        return;
    };
    watch(
        hub,
        program,
        home.join(".cursor"),
        Meters::new(Live::default()),
    )
    .await;
}

/// [`run`] with the CLI, its config folder and the meters' source given.
/// The folder only names the account; it isn't read.
async fn watch<S: Source>(hub: Hub, program: PathBuf, config_dir: PathBuf, mut meters: Meters<S>) {
    let mut commands = hub.commands();
    let mut shown = None;
    let mut listening = true;
    let mut tick = tokio::time::interval(CHECK_EVERY);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        // Whether to check, and whether you asked for it.
        let check = tokio::select! {
            _ = tick.tick() => Some(false),
            command = commands.recv(), if listening => match command {
                Ok(Command::RefreshUsage(None)) | Err(RecvError::Lagged(_)) => Some(true),
                Ok(Command::RefreshUsage(Some(id))) => (shown == Some(id)).then_some(true),
                Err(RecvError::Closed) => {
                    listening = false;
                    None
                }
            },
        };
        if let Some(asked) = check {
            shown = refresh(&hub, &program, &config_dir, &mut meters, asked)
                .await
                .or(shown);
        }
    }
}

/// Reads the login and, with Cursor usage on, its meters, and shows them,
/// or shows the account logged out while the CLI is signed out. Returns
/// the account shown, or none when the read failed and what shows stays.
async fn refresh<S: Source>(
    hub: &Hub,
    program: &Path,
    config_dir: &Path,
    meters: &mut Meters<S>,
    asked: bool,
) -> Option<AccountId> {
    let device = hub.device().id;
    let account = match read(program).await {
        Ok(Some(login)) => {
            let usage = if usage_on(hub) {
                Some(meters.read(&login, asked).await)
            } else {
                meters.forget();
                None
            };
            let mut account = account(device, config_dir, login);
            match usage {
                Some(Ok(usage)) => account.usage = Some(usage),
                Some(Err(failure)) => account.usage_failure = Some(failure),
                None => {}
            }
            account
        }
        // The account stays, so you can sign it in again from MaxHQ.
        Ok(None) => {
            meters.forget();
            let mut account = account(device, config_dir, Login::default());
            account.usage_failure = Some(UsageFailure {
                at: Timestamp::now(),
                reason: LOGGED_OUT.to_owned(),
            });
            account
        }
        Err(reason) => {
            debug!(%reason, "couldn't read the Cursor login");
            return None;
        }
    };
    let id = account.id;
    hub.apply([Change::Upsert {
        value: Entity::Account(account),
    }]);
    Some(id)
}

/// Whether you turned Cursor usage on for this device.
fn usage_on(hub: &Hub) -> bool {
    matches!(
        hub.get(&EntityKey::DeviceSettings(hub.device().id)),
        Some(Entity::DeviceSettings(settings)) if settings.cursor_usage
    )
}

/// The account a CLI login on `device` has.
fn account_id(device: DeviceId, config_dir: &Path) -> AccountId {
    names::account(device, Vendor::Cursor, &config_dir.to_string_lossy())
}

/// The account entity for `login`, with no usage.
fn account(device: DeviceId, config_dir: &Path, login: Login) -> Account {
    Account {
        id: account_id(device, config_dir),
        device,
        vendor: Vendor::Cursor,
        label: LABEL.to_owned(),
        login: LoginKind::ConfigDir,
        origin: None,
        config_dir: config_dir.to_string_lossy().into_owned(),
        email: login.email,
        organization: None,
        plan: login.plan,
        models: Vec::new(),
        usage: None,
        usage_failure: None,
        acp: None,
        model_server: None,
    }
}

/// Who the CLI is signed in as, or none when it's signed out.
async fn read(program: &Path) -> Result<Option<Login>, String> {
    let status = call(program, "status").await?;
    if status["isAuthenticated"].as_bool() != Some(true) {
        return Ok(None);
    }
    // The plan is extra, so a failed `about` still shows the login.
    let about = call(program, "about").await.unwrap_or(Value::Null);
    let text = |value: &Value| {
        value
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    Ok(Some(Login {
        email: text(&status["userInfo"]["email"]).or_else(|| text(&about["userEmail"])),
        plan: text(&about["subscriptionTier"]),
    }))
}

/// Runs `agent <what> --format json` and reads what it prints. A key from
/// `maxd`'s own environment is left out, so the CLI reports its own login.
async fn call(program: &Path, what: &str) -> Result<Value, String> {
    let run = crate::tools::retry_busy(|| async {
        Process::new(program)
            .args([what, "--format", "json"])
            .env_remove(cursor::API_KEY)
            .env_remove(cursor::AUTH_TOKEN)
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
    });
    let output = tokio::time::timeout(TIMEOUT, run)
        .await
        .map_err(|_| format!("`{what}` took too long"))?
        .map_err(|err| format!("couldn't run `{what}`: {err}"))?;
    if !output.status.success() {
        return Err(format!("`{what}` failed with {}", output.status));
    }
    serde_json::from_slice(&output.stdout).map_err(|err| format!("`{what}` printed no JSON: {err}"))
}

// The fake CLI is an sh script.
#[cfg(all(test, unix))]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use maxhq_protocol::model::{UsagePeriod, UsageSource};
    use serde_json::json;

    use super::*;
    use crate::hub;
    use crate::secrets::testing::hub;
    use crate::testing::TempDir;

    /// A stand-in CLI that prints `status` and `about` as given.
    fn fake(temp: &TempDir, status: &str, about: &str) -> PathBuf {
        let program = temp.path().join("cursor-agent");
        let script = format!(
            "#!/bin/sh\n\
             [ \"$2 $3\" = \"--format json\" ] || exit 2\n\
             [ -z \"$CURSOR_API_KEY\" ] || exit 3\n\
             case \"$1\" in\n\
             status) printf '%s' '{status}' ;;\n\
             about) printf '%s' '{about}' ;;\n\
             *) exit 2 ;;\n\
             esac\n"
        );
        fs::write(&program, script).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        program
    }

    const STATUS: &str = r#"{"status":"authenticated","isAuthenticated":true,"userInfo":{"email":"someone@example.com","userId":1}}"#;
    const ABOUT: &str =
        r#"{"cliVersion":"2026.10.22","subscriptionTier":"Pro","userEmail":"someone@example.com"}"#;

    /// A stand-in Keychain and Cursor that give what the test queues, and
    /// count what was asked.
    #[derive(Default)]
    struct Stub {
        tokens: VecDeque<keychain::Read>,
        answers: VecDeque<Result<Value, Failure>>,
        reads: usize,
        sent: Vec<String>,
    }

    impl Stub {
        fn token(mut self, token: &str) -> Self {
            self.tokens
                .push_back(Ok(Some(Zeroizing::new(token.to_owned()))));
            self
        }

        fn answer(mut self, answer: Result<Value, Failure>) -> Self {
            self.answers.push_back(answer);
            self
        }
    }

    impl Source for Stub {
        async fn token(&mut self) -> keychain::Read {
            self.reads += 1;
            self.tokens
                .pop_front()
                .expect("an unexpected Keychain read")
        }

        async fn period(&mut self, token: &str) -> Result<Value, Failure> {
            self.sent.push(token.to_owned());
            self.answers
                .pop_front()
                .expect("an unexpected call to Cursor")
        }
    }

    /// A recorded month.
    fn month() -> Value {
        json!({
            "billingCycleEnd": "1789876386000",
            "planUsage": { "totalPercentUsed": 72.4, "autoPercentUsed": 69.5, "apiPercentUsed": 100 }
        })
    }

    fn shown(hub: &Hub, config_dir: &Path) -> Option<Account> {
        match hub.get(&EntityKey::Account(account_id(hub.device().id, config_dir))) {
            Some(Entity::Account(account)) => Some(account),
            _ => None,
        }
    }

    /// Turns Cursor usage on or off for the hub's device.
    fn set_usage(hub: &Hub, on: bool) {
        let mut settings = hub::default_settings(hub.device());
        settings.cursor_usage = on;
        hub.apply([Change::Upsert {
            value: Entity::DeviceSettings(settings),
        }]);
    }

    /// A CLI signed in as someone, with somewhere for its account.
    struct Setup {
        temp: TempDir,
        hub: Hub,
        program: PathBuf,
        config_dir: PathBuf,
    }

    impl Setup {
        fn new() -> Self {
            let temp = TempDir::new();
            let program = fake(&temp, STATUS, ABOUT);
            let config_dir = temp.path().join(".cursor");
            Self {
                temp,
                hub: hub(),
                program,
                config_dir,
            }
        }

        async fn check(&self, meters: &mut Meters<Stub>, asked: bool) -> Account {
            refresh(&self.hub, &self.program, &self.config_dir, meters, asked).await;
            shown(&self.hub, &self.config_dir).expect("the account shows")
        }
    }

    #[tokio::test]
    async fn a_signed_in_cli_shows_its_email_and_plan() {
        let temp = TempDir::new();
        let program = fake(&temp, STATUS, ABOUT);
        let hub = hub();
        let config_dir = temp.path().join(".cursor");
        let mut meters = Meters::new(Stub::default());
        assert!(refresh(&hub, &program, &config_dir, &mut meters, false)
            .await
            .is_some());

        let account = shown(&hub, &config_dir).expect("the account shows");
        assert_eq!(account.vendor, Vendor::Cursor);
        assert_eq!(account.login, LoginKind::ConfigDir);
        assert_eq!(account.label, "Cursor");
        assert_eq!(account.email.as_deref(), Some("someone@example.com"));
        assert_eq!(account.plan.as_deref(), Some("Pro"));
        assert_eq!(account.usage, None);
        assert_eq!(account.usage_failure, None);
        assert_eq!(meters.source.reads, 0, "usage is off, so no Keychain");
        assert!(!config_dir.exists(), "the folder isn't touched");
    }

    #[tokio::test]
    async fn a_failed_about_still_shows_the_login() {
        let temp = TempDir::new();
        let program = fake(&temp, STATUS, "not json");
        let hub = hub();
        let config_dir = temp.path().join(".cursor");
        refresh(
            &hub,
            &program,
            &config_dir,
            &mut Meters::new(Stub::default()),
            false,
        )
        .await;
        let account = shown(&hub, &config_dir).expect("the account shows");
        assert_eq!(account.email.as_deref(), Some("someone@example.com"));
        assert_eq!(account.plan, None);
    }

    /// What `agent status` says while the CLI is signed out.
    const SIGNED_OUT: &str = r#"{"status":"unauthenticated","isAuthenticated":false}"#;

    #[tokio::test]
    async fn signing_out_keeps_the_account_logged_out() {
        let setup = Setup::new();
        set_usage(&setup.hub, true);
        let stub = Stub::default().token("keychain-token").answer(Ok(month()));
        let mut meters = Meters::new(stub);
        let before = setup.check(&mut meters, false).await;
        assert!(before.usage.is_some());

        let program = fake(&setup.temp, SIGNED_OUT, ABOUT);
        let id = refresh(&setup.hub, &program, &setup.config_dir, &mut meters, true).await;
        assert_eq!(id, Some(before.id), "a sign-in can still refresh it");
        let after = shown(&setup.hub, &setup.config_dir).expect("the account stays");
        assert_eq!(
            (&after.label, &after.config_dir, after.login),
            (&before.label, &before.config_dir, LoginKind::ConfigDir)
        );
        assert_eq!((after.email, after.plan, after.usage), (None, None, None));
        assert_eq!(after.usage_failure.unwrap().reason, "logged out");
        assert!(meters.token.is_none(), "the old login isn't kept");
        assert_eq!(meters.source.sent.len(), 1, "Cursor isn't asked");
    }

    #[tokio::test]
    async fn a_cli_that_never_signed_in_shows_logged_out() {
        let temp = TempDir::new();
        let hub = hub();
        let config_dir = temp.path().join(".cursor");
        let program = fake(&temp, SIGNED_OUT, ABOUT);
        let at = Timestamp::now();
        let mut meters = Meters::new(Stub::default());
        let id = refresh(&hub, &program, &config_dir, &mut meters, false).await;
        let account = shown(&hub, &config_dir).expect("the account shows");
        assert_eq!(id, Some(account.id));
        assert_eq!(account.label, "Cursor");
        let failure = account.usage_failure.expect("it shows logged out");
        assert_eq!(failure.reason, "logged out");
        assert!(failure.at >= at);
        assert_eq!(meters.source.reads, 0, "no Keychain");
        assert!(!config_dir.exists(), "the folder isn't touched");
    }

    #[tokio::test]
    async fn a_cli_that_fails_leaves_what_shows() {
        let temp = TempDir::new();
        let hub = hub();
        let config_dir = temp.path().join(".cursor");
        let mut meters = Meters::new(Stub::default());
        refresh(
            &hub,
            &fake(&temp, STATUS, ABOUT),
            &config_dir,
            &mut meters,
            false,
        )
        .await;
        assert!(refresh(
            &hub,
            &fake(&temp, "oops", ABOUT),
            &config_dir,
            &mut meters,
            false
        )
        .await
        .is_none());
        assert!(shown(&hub, &config_dir).is_some());
    }

    #[tokio::test]
    async fn with_usage_on_the_month_shows_from_the_keychain_login() {
        let setup = Setup::new();
        set_usage(&setup.hub, true);
        let stub = Stub::default()
            .token("keychain-token")
            .answer(Ok(month()))
            .answer(Ok(month()));
        let mut meters = Meters::new(stub);

        let account = setup.check(&mut meters, false).await;
        let usage = account.usage.expect("the month shows");
        assert_eq!(usage.source, UsageSource::CursorDashboard);
        assert_eq!(usage.windows.len(), 3);
        assert!(usage
            .windows
            .iter()
            .all(|w| w.period == UsagePeriod::BillingCycle));
        assert_eq!(account.plan.as_deref(), Some("Pro"));
        assert_eq!(account.usage_failure, None);

        // The login is kept, so the Keychain is read once.
        setup.check(&mut meters, false).await;
        assert_eq!(meters.source.reads, 1);
        assert_eq!(meters.source.sent, ["keychain-token", "keychain-token"]);
    }

    #[tokio::test]
    async fn a_failed_check_falls_back_to_the_plan_with_a_reason() {
        let setup = Setup::new();
        set_usage(&setup.hub, true);
        let stub = Stub::default()
            .token("keychain-token")
            .answer(Ok(month()))
            .answer(Err(Failure::Other("Cursor answered 500".into())));
        let mut meters = Meters::new(stub);
        assert!(setup.check(&mut meters, false).await.usage.is_some());

        let account = setup.check(&mut meters, true).await;
        assert_eq!(account.usage, None);
        assert_eq!(account.plan.as_deref(), Some("Pro"));
        assert_eq!(account.usage_failure.unwrap().reason, "Cursor answered 500");
    }

    #[tokio::test]
    async fn a_missing_or_unreadable_login_says_why() {
        let setup = Setup::new();
        set_usage(&setup.hub, true);
        let mut stub = Stub::default();
        stub.tokens.push_back(Ok(None));
        stub.tokens
            .push_back(Err("macOS didn't allow reading the Keychain".into()));
        let mut meters = Meters::new(stub);

        let failure = setup.check(&mut meters, false).await.usage_failure.unwrap();
        assert_eq!(
            failure.reason,
            "the Cursor CLI has no login in the Keychain"
        );
        let failure = setup.check(&mut meters, true).await.usage_failure.unwrap();
        assert_eq!(failure.reason, "macOS didn't allow reading the Keychain");
        assert!(meters.source.sent.is_empty(), "Cursor isn't asked");
    }

    #[tokio::test]
    async fn a_turned_down_login_is_read_again() {
        let setup = Setup::new();
        set_usage(&setup.hub, true);
        let stub = Stub::default()
            .token("old-token")
            .answer(Err(Failure::Rejected))
            .token("new-token")
            .answer(Ok(month()));
        let mut meters = Meters::new(stub);
        let failure = setup.check(&mut meters, false).await.usage_failure.unwrap();
        assert_eq!(failure.reason, "Cursor turned down the saved login");

        assert!(setup.check(&mut meters, true).await.usage.is_some());
        assert_eq!(meters.source.sent, ["old-token", "new-token"]);
    }

    #[tokio::test]
    async fn failures_back_off_on_the_timer_but_not_when_you_ask() {
        let setup = Setup::new();
        set_usage(&setup.hub, true);
        let down = || Err(Failure::Other("couldn't reach Cursor".into()));
        let stub = Stub::default()
            .token("keychain-token")
            .answer(down())
            .answer(down())
            .answer(down())
            .answer(down());
        let mut meters = Meters::new(stub);
        let calls = |meters: &Meters<Stub>| meters.source.sent.len();

        setup.check(&mut meters, false).await;
        assert_eq!(calls(&meters), 1);
        // One failure skips one check on the timer, and still shows why.
        let skipped = setup.check(&mut meters, false).await;
        assert_eq!(calls(&meters), 1);
        assert!(skipped.usage_failure.is_some());
        setup.check(&mut meters, false).await;
        assert_eq!(calls(&meters), 2);
        // Two in a row skip three.
        for _ in 0..3 {
            setup.check(&mut meters, false).await;
        }
        assert_eq!(calls(&meters), 2);
        // Asking runs it now.
        setup.check(&mut meters, true).await;
        assert_eq!(calls(&meters), 3);
        assert_eq!(meters.skip, 7);
        // However long it fails, it tries again within the hour.
        meters.failures = 20;
        setup.check(&mut meters, true).await;
        assert_eq!(meters.skip, MOST_SKIPPED);
    }

    #[tokio::test]
    async fn turning_usage_off_or_a_new_login_forgets_the_old_one() {
        let setup = Setup::new();
        set_usage(&setup.hub, true);
        let stub = Stub::default()
            .token("first-token")
            .answer(Ok(month()))
            .token("second-token")
            .answer(Ok(month()))
            .token("third-token")
            .answer(Ok(month()));
        let mut meters = Meters::new(stub);
        setup.check(&mut meters, false).await;

        set_usage(&setup.hub, false);
        let account = setup.check(&mut meters, true).await;
        assert_eq!((account.usage, account.usage_failure), (None, None));
        assert!(meters.token.is_none(), "the login isn't kept");

        set_usage(&setup.hub, true);
        setup.check(&mut meters, true).await;
        let other = r#"{"isAuthenticated":true,"userInfo":{"email":"other@example.com"}}"#;
        let program = fake(&setup.temp, other, ABOUT);
        refresh(&setup.hub, &program, &setup.config_dir, &mut meters, false).await;
        assert_eq!(
            meters.source.sent,
            ["first-token", "second-token", "third-token"]
        );
    }
}
