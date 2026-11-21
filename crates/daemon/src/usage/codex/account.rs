// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Keeps one Codex login's account entity current: who's logged in, the
//! plan, and the rate limits. Each check starts an app-server, reads, and
//! stops it again, unless an agent's app-server for the login read the
//! limits lately, which shows them already. The first check also lists
//! the models, so the app can offer them before an agent runs.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use maxhq_protocol::ids::AccountId;
use maxhq_protocol::model::{Account, LoginKind, ModelChoice, UsageFailure, Vendor};
use maxhq_protocol::state::{Change, Entity};
use maxhq_protocol::time::Timestamp;
use serde_json::{json, Value};
use tokio::sync::{broadcast, watch};

use super::app_server::{AppServer, Error, Method};
use super::limits::{self, Reading};
use super::models;
use super::{Home, ROLLOUTS};
use crate::hub::{Command, Hub};
use crate::service::find_program;

/// How often the limits are read.
pub const CHECK_EVERY: Duration = Duration::from_secs(5 * 60);

/// The first wait before starting a failed app-server again. Each failure
/// in a row doubles it, up to [`LAST_RETRY`].
const FIRST_RETRY: Duration = Duration::from_secs(5);

/// The longest wait between tries.
const LAST_RETRY: Duration = CHECK_EVERY;

/// How recent an agent's reading of the limits has to be for a check to
/// skip its own: a check apart, and a minute for its app-server to answer.
const FRESH: Duration = Duration::from_secs(CHECK_EVERY.as_secs() + 60);

/// What the app shows before the first check, and keeps unless you rename
/// the account.
const LABEL: &str = "Codex";

/// The `codex` executable, from `PATH` or a usual install folder.
pub fn find_codex() -> Option<PathBuf> {
    find_program("codex")
}

/// The account entity before anything is known about the login.
pub fn initial(home: &Home, hub: &Hub) -> Account {
    let device = hub.device().id;
    Account {
        id: home.account(device),
        device,
        vendor: Vendor::Codex,
        label: LABEL.to_owned(),
        config_dir: home.config_dir(),
        email: None,
        organization: None,
        plan: None,
        models: Vec::new(),
        usage: None,
        usage_failure: None,
        acp: None,
        login: LoginKind::ConfigDir,
        origin: None,
        model_server: None,
    }
}

/// What woke the account task.
enum Wake {
    /// Time to read the limits.
    Check,
    /// A rollout showed newer limits.
    Rollout,
    /// Nothing to act on.
    Nothing,
}

/// The account task's state between wakes.
struct Watch {
    hub: Hub,
    home: Home,
    account: Account,
    commands: broadcast::Receiver<Command>,
    rollouts: watch::Receiver<HashMap<AccountId, Reading>>,
    /// Whether the models were listed since the last failed start.
    listed: bool,
}

/// Keeps one login's account entity current until the process exits.
/// `program` finds the `codex` to run, and is [`find_codex`] outside tests.
pub async fn watch(hub: Hub, home: Home) {
    watch_with(hub, home, find_codex).await;
}

/// [`watch`] with the way to find `codex` given, for tests.
async fn watch_with(hub: Hub, home: Home, program: impl Fn() -> Option<PathBuf>) {
    let account = initial(&home, &hub);
    let mut task = Watch {
        commands: hub.commands(),
        rollouts: ROLLOUTS.subscribe(),
        hub,
        home,
        account,
        listed: false,
    };
    task.take_rollout();
    task.publish();
    let mut retry = FIRST_RETRY;
    let mut asked = false;
    loop {
        let wait = if !asked && task.read_lately() {
            CHECK_EVERY
        } else {
            match task.read(&program).await {
                Ok(()) => {
                    retry = FIRST_RETRY;
                    CHECK_EVERY
                }
                Err(error) => {
                    task.fail(&error);
                    if error.needs_restart() {
                        task.listed = false;
                        let wait = retry;
                        retry = (retry * 2).min(LAST_RETRY);
                        wait
                    } else {
                        CHECK_EVERY
                    }
                }
            }
        };
        asked = task.wait(wait).await;
    }
}

impl Watch {
    /// Starts an app-server, reads the login and its limits, and the models
    /// when they weren't listed yet, then stops it.
    async fn read(&mut self, program: &impl Fn() -> Option<PathBuf>) -> Result<(), Error> {
        let program = program().ok_or(Error::NotFound)?;
        let mut server = AppServer::start(&program, &self.home.dir).await?;
        let read = self.read_from(&mut server).await;
        server.stop().await;
        read
    }

    /// [`Watch::read`] through an app-server that started.
    async fn read_from(&mut self, server: &mut AppServer) -> Result<(), Error> {
        if !self.listed {
            match list_models(server).await {
                Ok(models) => {
                    self.hub.set_models(self.account.id, models);
                    self.listed = true;
                }
                Err(error) if error.needs_restart() => return Err(error),
                Err(error) => tracing::debug!(%error, "codex model/list failed"),
            }
        }
        check(server, &mut self.account).await?;
        self.publish();
        Ok(())
    }

    /// Whether an agent's app-server for the login read the limits within
    /// the last check and a bit, as it does every few minutes while it
    /// runs. The account shows that reading, so no check is needed.
    fn read_lately(&self) -> bool {
        let Some(at) = self.hub.usage_reported_at(self.account.id) else {
            return false;
        };
        let lately = u64::try_from(FRESH.as_millis()).unwrap_or(u64::MAX);
        at.0.saturating_add(lately) > Timestamp::now().0
    }

    /// Waits before the next check. A request to refresh ends the wait
    /// early and returns true, and rollouts still count meanwhile.
    async fn wait(&mut self, duration: Duration) -> bool {
        let deadline = tokio::time::sleep(duration);
        tokio::pin!(deadline);
        loop {
            let wake = tokio::select! {
                () = &mut deadline => return false,
                command = self.commands.recv() => self.command(command),
                changed = self.rollouts.changed() => match changed {
                    Ok(()) => Wake::Rollout,
                    Err(_) => Wake::Nothing,
                },
            };
            match wake {
                Wake::Check => return true,
                Wake::Rollout => {
                    self.take_rollout();
                    self.publish();
                }
                Wake::Nothing => {}
            }
        }
    }

    /// What a hub command asks of this account.
    fn command(&self, command: Result<Command, broadcast::error::RecvError>) -> Wake {
        match command {
            Ok(Command::RefreshUsage(None)) => Wake::Check,
            Ok(Command::RefreshUsage(Some(id))) if id == self.account.id => Wake::Check,
            Ok(Command::RefreshUsage(Some(_))) => Wake::Nothing,
            // A missed refresh might have been for this account.
            Err(broadcast::error::RecvError::Lagged(_)) => Wake::Check,
            Err(broadcast::error::RecvError::Closed) => Wake::Nothing,
        }
    }

    /// Shows the latest rollout reading, when it's newer than what shows.
    fn take_rollout(&mut self) {
        let reading = self
            .rollouts
            .borrow_and_update()
            .get(&self.account.id)
            .cloned();
        if let Some(reading) = reading {
            self.account.usage = limits::merge(self.account.usage.take(), &reading);
        }
    }

    /// Shows why the last check failed, and keeps the last good usage.
    fn fail(&mut self, error: &Error) {
        tracing::debug!(config_dir = %self.account.config_dir, %error, "codex usage check failed");
        if matches!(error, Error::LoggedOut) {
            self.account.email = None;
            self.account.plan = None;
        }
        self.account.usage_failure = Some(UsageFailure {
            at: Timestamp::now(),
            reason: error.to_string(),
        });
        self.publish();
    }

    /// Sends the account entity to the hub.
    fn publish(&self) {
        self.hub.apply([Change::Upsert {
            value: Entity::Account(self.account.clone()),
        }]);
    }
}

/// Reads the login and its limits into `account`. Usage from before stays
/// when the check fails.
pub async fn check(server: &mut AppServer, account: &mut Account) -> Result<(), Error> {
    let read = server
        .call(Method::ReadAccount, json!({ "refreshToken": false }))
        .await?;
    let (email, plan) = login(&read)?;
    account.email = email;
    account.plan = plan;
    let limits = server
        .call(
            Method::ReadRateLimits,
            json!({ "excludeResetCreditDetails": true }),
        )
        .await?;
    let reading = limits::from_app_server(&limits, Timestamp::now());
    account.usage = limits::merge(account.usage.take(), &reading);
    account.usage_failure = None;
    Ok(())
}

/// Every page of `model/list`.
pub async fn list_models(server: &mut AppServer) -> Result<Vec<ModelChoice>, Error> {
    let mut list = Vec::new();
    let mut params = json!({});
    while let Some(cursor) =
        models::page(&server.call(Method::ListModels, params).await?, &mut list)
    {
        params = json!({ "cursor": cursor });
    }
    Ok(list)
}

/// The email and plan in an `account/read` result. An API key login has
/// neither.
fn login(read: &Value) -> Result<(Option<String>, Option<String>), Error> {
    let account = &read["account"];
    if account.is_null() {
        return Err(Error::LoggedOut);
    }
    if account["type"].as_str() != Some("chatgpt") {
        return Ok((None, None));
    }
    let text = |key: &str| account[key].as_str().map(str::to_owned);
    Ok((text("email"), text("planType")))
}

#[cfg(test)]
mod tests {
    use super::super::app_server::tests::FakeHome;
    use super::*;
    use maxhq_protocol::ids::{BootId, DeviceId};
    use maxhq_protocol::model::{Device, Usage, UsageSource};

    fn hub() -> Hub {
        Hub::new(
            Device {
                id: DeviceId::derive("test"),
                name: "mac".into(),
                os: "macos".into(),
                boot_id: BootId("b".into()),
                maxd_version: "0.1.0".into(),
                winding_down_until: None,
                workspace_host: None,
                build: None,
                target: None,
            },
            HashMap::new(),
        )
    }

    fn home(fake: &FakeHome) -> Home {
        Home {
            dir: fake.home.clone(),
            real: fake.home.canonicalize().unwrap(),
        }
    }

    #[tokio::test]
    async fn a_check_reads_the_login_and_the_limits() {
        let fake = FakeHome::new("codex-check");
        let hub = hub();
        let mut account = initial(&home(&fake), &hub);
        let mut server = AppServer::start(&fake.program, &fake.home).await.unwrap();
        check(&mut server, &mut account).await.unwrap();
        assert_eq!(account.label, "Codex");
        assert_eq!(account.email.as_deref(), Some("dev@example.com"));
        assert_eq!(account.plan.as_deref(), Some("plus"));
        assert_eq!(account.config_dir, fake.home.to_string_lossy());
        let usage = account.usage.as_ref().unwrap();
        assert_eq!(usage.source, UsageSource::CodexAppServer);
        assert_eq!(usage.windows.len(), 3);
        assert_eq!(account.usage_failure, None);
    }

    #[tokio::test]
    async fn a_failed_check_keeps_the_last_good_usage() {
        let fake = FakeHome::new("codex-check-fails");
        let hub = hub();
        let mut account = initial(&home(&fake), &hub);
        let mut server = AppServer::start(&fake.program, &fake.home).await.unwrap();
        check(&mut server, &mut account).await.unwrap();
        let good = account.usage.clone();

        fake.remove("limits.json");
        let error = check(&mut server, &mut account).await.unwrap_err();
        assert!(!error.needs_restart());
        assert_eq!(account.usage, good);

        fake.write(
            "account.json",
            &json!({ "account": null, "requiresOpenaiAuth": true }),
        );
        let error = check(&mut server, &mut account).await.unwrap_err();
        assert!(matches!(error, Error::LoggedOut));
        assert_eq!(account.usage, good);
    }

    #[test]
    fn api_key_logins_have_no_email_or_plan() {
        let read = json!({ "account": { "type": "apiKey" }, "requiresOpenaiAuth": true });
        assert_eq!(login(&read).unwrap(), (None, None));
    }

    /// The account entity once a check has worked or failed.
    async fn account_in(hub: &Hub, home: &Home) -> Option<Account> {
        let id = home.account(hub.device().id);
        for _ in 0..200 {
            let found = hub
                .snapshot()
                .entities
                .into_iter()
                .find_map(|entity| match entity {
                    Entity::Account(account) if account.id == id => Some(account),
                    _ => None,
                });
            if let Some(account) = found
                && (account.usage.is_some() || account.usage_failure.is_some())
            {
                return Some(account);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        None
    }

    #[tokio::test]
    async fn the_task_publishes_the_account_and_failures() {
        let fake = FakeHome::new("codex-task");
        let hub = hub();
        let home = home(&fake);
        let program = fake.program.clone();
        let task = tokio::spawn(watch_with(hub.clone(), home.clone(), move || {
            Some(program.clone())
        }));
        let account = account_in(&hub, &home).await.expect("an account");
        assert_eq!(account.plan.as_deref(), Some("plus"));
        assert!(account.usage.is_some());
        task.abort();

        let missing = FakeHome::new("codex-task-missing");
        let hub = self::hub();
        let home = self::home(&missing);
        let task = tokio::spawn(watch_with(hub.clone(), home.clone(), || None));
        let account = account_in(&hub, &home).await.expect("an account");
        assert_eq!(
            account
                .usage_failure
                .map(|failure| failure.reason)
                .as_deref(),
            Some("codex not found")
        );
        assert_eq!(account.usage, None);
        task.abort();
    }

    #[tokio::test]
    async fn the_first_check_lists_every_page_of_models_and_each_check_stops_its_app_server() {
        let fake = FakeHome::new("codex-task-models");
        let hub = hub();
        let home = home(&fake);
        let program = fake.program.clone();
        let task = tokio::spawn(watch_with(hub.clone(), home.clone(), move || {
            Some(program.clone())
        }));
        let account = account_in(&hub, &home).await.expect("an account");
        let models: Vec<(&str, bool)> = account
            .models
            .iter()
            .map(|model| (model.id.as_str(), model.default))
            .collect();
        assert_eq!(models, [("gpt-6.1-sol", true), ("gpt-5.6-luna", false)]);
        assert_eq!(account.models[0].label, "GPT-6.1 Sol");

        let sent = |method: &str| {
            fake.requests()
                .iter()
                .filter(|request| request["method"] == method)
                .count()
        };
        hub.send(Command::RefreshUsage(None));
        for _ in 0..200 {
            if sent("account/rateLimits/read") >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(sent("account/rateLimits/read"), 2);
        assert_eq!(sent("model/list"), 2, "a check doesn't list them again");
        assert_eq!(sent("initialize"), 2, "each check starts its own");
        for _ in 0..200 {
            if fake.stops() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(fake.stops(), 2, "each check stops its app-server");
        task.abort();
    }

    #[tokio::test]
    async fn a_fresh_reading_from_an_agent_skips_the_check_until_a_refresh() {
        let fake = FakeHome::new("codex-task-fresh");
        let hub = hub();
        let home = home(&fake);
        let usage = Usage {
            checked_at: Timestamp::now(),
            source: UsageSource::CodexAppServer,
            windows: Vec::new(),
            reset_credits: None,
        };
        hub.set_usage(home.account(hub.device().id), usage);
        let program = fake.program.clone();
        let task = tokio::spawn(watch_with(hub.clone(), home.clone(), move || {
            Some(program.clone())
        }));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            fake.requests().is_empty(),
            "an agent's app-server read them just now"
        );

        hub.send(Command::RefreshUsage(None));
        let read = |method: &str| {
            fake.requests()
                .iter()
                .any(|request| request["method"] == method)
        };
        for _ in 0..200 {
            if read("account/rateLimits/read") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(read("account/rateLimits/read"), "a refresh always reads");
        task.abort();
    }

    /// Reads the real `~/.codex` once, through the real `codex app-server`,
    /// and prints the plan and windows without the email. Run it with
    /// `cargo test -p maxd -- --ignored real_codex_usage --nocapture`.
    #[tokio::test]
    #[ignore = "reads the real ~/.codex through codex app-server"]
    async fn real_codex_usage() {
        let home = super::super::homes().into_iter().next().expect("~/.codex");
        let program = find_codex().expect("codex on PATH");
        let hub = hub();
        let mut account = initial(&home, &hub);
        let mut server = AppServer::start(&program, &home.dir).await.unwrap();
        check(&mut server, &mut account).await.unwrap();
        drop(server);
        println!("codex: {}", program.display());
        println!("config_dir: {}", account.config_dir);
        println!("logged in: {}", account.email.is_some());
        println!("plan: {:?}", account.plan);
        let usage = account.usage.unwrap();
        println!(
            "source: {:?}, checked_at: {}",
            usage.source, usage.checked_at.0
        );
        for window in usage.windows {
            println!(
                "scope {:?}, {} minutes, {}% used, resets at {:?}",
                window.scope,
                window.window_minutes,
                window.used_percent,
                window.resets_at.map(|at| at.0 / 1000)
            );
        }
    }
}
