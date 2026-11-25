// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Usage for Claude logins, per decision P-4.
//!
//! Each config folder is an account. A check asks `claude auth status` who
//! is logged in, then takes the usage Claude Code cached in the login's
//! state file when that's under an hour old, and runs the usage probe when
//! it isn't. Checks run every 5 minutes and whenever a client sends
//! `RefreshUsage`.

mod cache;
mod cli;
mod windows;

use std::time::Duration;

use maxhq_protocol::ids::DeviceId;
use maxhq_protocol::model::{Account, LoginKind, Usage, UsageFailure, Vendor};
use maxhq_protocol::state::{Change, Entity};
use maxhq_protocol::time::Timestamp;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::MissedTickBehavior;

use crate::hub::{Command, Hub};
use crate::observe::claude::config::{self, ConfigDir};
use cli::{Auth, Cli, Identity};

pub use cli::is_probe;

/// How often each login is checked.
const EVERY: Duration = Duration::from_secs(5 * 60);

/// The label an account gets until you name it.
const LABEL: &str = "Claude";

/// Keeps this vendor's account entities current until the process exits.
/// Returns at once when this device has no Claude config folder.
pub async fn run(hub: Hub) {
    let configs = config::config_dirs();
    if configs.is_empty() {
        return;
    }
    let mut commands = hub.commands();
    let device = hub.device().id;
    let mut logins: Vec<Login> = configs
        .into_iter()
        .map(|config| Login::new(device, config))
        .collect();
    // The accounts show up before their first check ends.
    hub.apply(logins.iter().map(Login::change));

    let mut listening = true;
    let mut tick = tokio::time::interval(EVERY);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        let wanted = tokio::select! {
            _ = tick.tick() => Some(None),
            command = commands.recv(), if listening => match command {
                Ok(Command::RefreshUsage(account)) => Some(account),
                // Requests went missing, and checking every login covers them.
                Err(RecvError::Lagged(_)) => Some(None),
                Err(RecvError::Closed) => {
                    listening = false;
                    None
                }
            },
        };
        let Some(wanted) = wanted else {
            continue;
        };
        // Looked up each time, so a `claude` installed later is found.
        let claude = Cli::find();
        for login in &mut logins {
            if wanted.is_none_or(|id| id == login.account.id) {
                login.check(claude.as_ref(), Timestamp::now()).await;
                hub.apply([login.change()]);
            }
        }
    }
}

/// One config folder's login and the account it shows as.
struct Login {
    /// The config folder.
    config: ConfigDir,
    /// The account as the hub has it.
    account: Account,
}

impl Login {
    /// The login in `config` on `device`, not checked yet.
    fn new(device: DeviceId, config: ConfigDir) -> Self {
        let account = Account {
            id: config.account(device),
            device,
            vendor: Vendor::Claude,
            label: LABEL.to_owned(),
            config_dir: config.path.to_string_lossy().into_owned(),
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
        };
        Self { config, account }
    }

    /// The change that puts this account in the hub.
    fn change(&self) -> Change {
        Change::Upsert {
            value: Entity::Account(self.account.clone()),
        }
    }

    /// Checks who's logged in and reads the usage, running `claude` when
    /// there's one to run. `now` is when the check started.
    async fn check(&mut self, claude: Option<&Cli>, now: Timestamp) {
        let result = self.read(claude, now).await;
        let failed_at = Timestamp::now();
        if let Err(reason) = &result {
            tracing::debug!(%reason, config = %self.config.path.display(), "a Claude usage check failed");
        }
        record(&mut self.account, result, failed_at);
    }

    /// The usage of this login: the cached reading when it's fresh, and the
    /// probe's otherwise.
    async fn read(&mut self, claude: Option<&Cli>, now: Timestamp) -> Result<Usage, String> {
        if let Some(claude) = claude {
            match claude.auth_status(&self.config).await {
                Ok(Auth::LoggedIn(identity)) => self.identify(identity),
                Ok(Auth::LoggedOut) => {
                    self.identify(Identity::default());
                    return Err("logged out".to_owned());
                }
                // Who's logged in stays as last read, and the usage can
                // still be read.
                Err(reason) => tracing::debug!(%reason, "couldn't read a Claude login"),
            }
        }
        if let Ok(state) = tokio::fs::read(&self.config.state_file).await
            && let Some(usage) = cache::cached(&state, now)
        {
            return Ok(usage);
        }
        let Some(claude) = claude else {
            return Err("claude not found".to_owned());
        };
        match claude.probe(&self.config).await {
            Ok(usage) => Ok(usage),
            // Decision P-4: a failed check tries again at once.
            Err(reason) => {
                tracing::debug!(%reason, "the Claude usage probe failed, so it runs again");
                claude.probe(&self.config).await
            }
        }
    }

    /// Takes in who's logged in now. A different email is a different
    /// login, and the last one's numbers don't describe it.
    fn identify(&mut self, identity: Identity) {
        let account = &mut self.account;
        if account.email.is_some() && identity.email.is_some() && account.email != identity.email {
            account.usage = None;
            account.usage_failure = None;
        }
        account.email = identity.email;
        account.organization = identity.organization;
        account.plan = identity.plan;
    }
}

/// Folds a check's result into `account`. A reading replaces the one held
/// unless it's older. A failure at `now` keeps the last good reading beside
/// it, and shows only while it's newer than that reading.
fn record(account: &mut Account, result: Result<Usage, String>, now: Timestamp) {
    match result {
        Ok(usage) => {
            let newer = account
                .usage
                .as_ref()
                .is_none_or(|held| usage.checked_at >= held.checked_at);
            if newer {
                account.usage = Some(usage);
            }
            if let Some(held) = &account.usage {
                let checked_at = held.checked_at;
                account.usage_failure = account
                    .usage_failure
                    .take()
                    .filter(|failure| failure.at > checked_at);
            }
        }
        Err(reason) => {
            account.usage_failure = Some(UsageFailure { at: now, reason });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;

    use maxhq_protocol::ids::BootId;
    use maxhq_protocol::model::{Device, UsagePeriod, UsageSource, UsageWindow};

    use super::*;
    use crate::observe::claude::tests::{fixture, scratch, Scratch};
    use cli::tests::{fake_claude, ANSWERS, FAILS};

    fn device() -> DeviceId {
        DeviceId::derive("test")
    }

    /// A config folder whose state file holds the fixture's cached reading,
    /// fetched `age` ago.
    fn folder(age: Duration) -> (Scratch, Login) {
        let dir = scratch();
        let path = dir.join("claude");
        fs::create_dir_all(&path).unwrap();
        let fetched = Timestamp::now().0 - age.as_millis() as u64;
        let state = fs::read_to_string(fixture("config/.claude.json"))
            .unwrap()
            .replace("1792889000000", &fetched.to_string());
        fs::write(path.join(".claude.json"), state).unwrap();
        (dir, Login::new(device(), ConfigDir::custom(path)))
    }

    fn reading(checked_at: u64) -> Usage {
        Usage {
            checked_at: Timestamp(checked_at),
            source: UsageSource::ClaudeProbe,
            windows: vec![UsageWindow {
                scope: None,
                name: None,
                window_minutes: 300,
                used_percent: 10.0,
                resets_at: None,
                period: UsagePeriod::Rolling,
                spend: None,
            }],
            reset_credits: None,
        }
    }

    const FRESH: Duration = Duration::from_secs(10 * 60);
    const STALE: Duration = Duration::from_secs(2 * 60 * 60);

    #[tokio::test]
    async fn a_fresh_cache_comes_first() {
        let (dir, mut login) = folder(FRESH);
        // The probe would fail, so a good reading has to be the cache.
        let claude = fake_claude(&dir.join("bin"), "cli/auth-status.json", FAILS);
        login.check(Some(&claude), Timestamp::now()).await;
        let account = &login.account;
        let usage = account.usage.as_ref().expect("a reading");
        assert_eq!(usage.source, UsageSource::ClaudeCache);
        assert_eq!(usage.windows.len(), 3);
        assert_eq!(account.usage_failure, None);
        assert_eq!(account.email.as_deref(), Some("max@example.com"));
        assert_eq!(account.organization.as_deref(), Some("Example Org"));
        assert_eq!(account.plan.as_deref(), Some("max"));
        assert_eq!(account.label, "Claude");
        assert_eq!(account.vendor, Vendor::Claude);
        assert_eq!(account.config_dir, dir.join("claude").to_string_lossy());
    }

    #[tokio::test]
    async fn a_stale_cache_runs_the_probe() {
        let (dir, mut login) = folder(STALE);
        let claude = fake_claude(&dir.join("bin"), "cli/auth-status.json", ANSWERS);
        let before = Timestamp::now();
        login.check(Some(&claude), Timestamp::now()).await;
        let usage = login.account.usage.as_ref().expect("a reading");
        assert_eq!(usage.source, UsageSource::ClaudeProbe);
        assert!(usage.checked_at >= before);
        assert_eq!(login.account.usage_failure, None);
    }

    #[tokio::test]
    async fn a_failed_probe_keeps_the_last_good_reading() {
        let (dir, mut login) = folder(STALE);
        let good = reading(Timestamp::now().0 - 60_000);
        login.account.usage = Some(good.clone());
        let fails = fake_claude(&dir.join("fails"), "cli/auth-status.json", FAILS);
        login.check(Some(&fails), Timestamp::now()).await;
        assert_eq!(login.account.usage, Some(good.clone()));
        let failure = login.account.usage_failure.clone().expect("a failure");
        assert_eq!(
            failure.reason,
            "claude exited without answering the usage request"
        );
        assert!(failure.at > good.checked_at);

        let answers = fake_claude(&dir.join("answers"), "cli/auth-status.json", ANSWERS);
        login.check(Some(&answers), Timestamp::now()).await;
        let usage = login.account.usage.as_ref().unwrap();
        assert_eq!(usage.source, UsageSource::ClaudeProbe);
        assert!(usage.checked_at >= failure.at);
        assert_eq!(login.account.usage_failure, None);
    }

    #[tokio::test]
    async fn a_logged_out_login_keeps_its_last_reading() {
        let (dir, mut login) = folder(FRESH);
        let claude = fake_claude(&dir.join("bin"), "cli/auth-status.json", FAILS);
        login.check(Some(&claude), Timestamp::now()).await;
        let good = login.account.usage.clone().expect("a reading");

        let logged_out = fake_claude(&dir.join("out"), "cli/auth-logged-out.json", ANSWERS);
        login.check(Some(&logged_out), Timestamp::now()).await;
        assert_eq!(login.account.usage, Some(good));
        assert_eq!(login.account.email, None);
        assert_eq!(login.account.plan, None);
        assert_eq!(
            login.account.usage_failure.map(|failure| failure.reason),
            Some("logged out".to_owned())
        );
    }

    #[tokio::test]
    async fn without_claude_only_the_cache_is_read() {
        let (_dir, mut fresh) = folder(FRESH);
        fresh.check(None, Timestamp::now()).await;
        assert_eq!(
            fresh.account.usage.map(|usage| usage.source),
            Some(UsageSource::ClaudeCache)
        );

        let (_dir, mut stale) = folder(STALE);
        stale.check(None, Timestamp::now()).await;
        assert_eq!(stale.account.usage, None);
        assert_eq!(
            stale.account.usage_failure.map(|failure| failure.reason),
            Some("claude not found".to_owned())
        );
    }

    #[test]
    fn a_newer_reading_always_wins() {
        let mut account = Login::new(device(), ConfigDir::custom("/srv/claude".into())).account;
        record(&mut account, Ok(reading(2_000)), Timestamp(2_100));
        record(&mut account, Ok(reading(1_000)), Timestamp(2_200));
        assert_eq!(
            account.usage,
            Some(reading(2_000)),
            "an older one is ignored"
        );

        record(&mut account, Err("offline".into()), Timestamp(3_000));
        assert_eq!(account.usage, Some(reading(2_000)));
        assert_eq!(account.usage_failure.as_ref().unwrap().at, Timestamp(3_000));

        // A cached reading fetched before the failure leaves it showing.
        record(&mut account, Ok(reading(2_500)), Timestamp(3_100));
        assert_eq!(account.usage, Some(reading(2_500)));
        assert!(account.usage_failure.is_some());

        record(&mut account, Ok(reading(3_000)), Timestamp(3_200));
        assert_eq!(account.usage, Some(reading(3_000)));
        assert_eq!(account.usage_failure, None);
    }

    #[test]
    fn another_login_drops_the_last_ones_reading() {
        let mut login = Login::new(device(), ConfigDir::custom("/srv/claude".into()));
        let max = Identity {
            email: Some("max@example.com".into()),
            organization: None,
            plan: Some("max".into()),
        };
        login.identify(max.clone());
        login.account.usage = Some(reading(1_000));
        login.identify(max);
        assert!(login.account.usage.is_some(), "the same login keeps it");
        login.identify(Identity {
            email: Some("sam@example.com".into()),
            organization: None,
            plan: Some("pro".into()),
        });
        assert_eq!(login.account.usage, None);
        assert_eq!(login.account.plan.as_deref(), Some("pro"));
    }

    /// Hides who a login is, so the output can be shared.
    fn masked(value: &Option<String>) -> &'static str {
        if value.is_some() {
            "<set>"
        } else {
            "<none>"
        }
    }

    /// Checks this device's real Claude logins once, the cached reading
    /// first, and prints the accounts the hub ends up with. Emails and
    /// organizations are masked.
    #[tokio::test]
    #[ignore = "reads this device's real Claude config folders and may run claude"]
    async fn real_logins() {
        let hub = Hub::new(
            Device {
                id: device(),
                name: "this device".into(),
                os: std::env::consts::OS.into(),
                boot_id: BootId("test".into()),
                maxd_version: env!("CARGO_PKG_VERSION").into(),
                winding_down_until: None,
                workspace_host: None,
                build: None,
                target: None,
                wsl: None,
            },
            HashMap::new(),
        );
        let claude = Cli::find();
        println!("claude: {:?}", claude.as_ref().map(|_| "found"));
        for config in config::config_dirs() {
            let mut login = Login::new(device(), config);
            login.check(claude.as_ref(), Timestamp::now()).await;
            hub.apply([login.change()]);
        }
        for entity in hub.snapshot().entities {
            let Entity::Account(account) = entity else {
                continue;
            };
            println!(
                "account {} label={} email={} organization={} plan={:?}",
                Path::new(&account.config_dir).display(),
                account.label,
                masked(&account.email),
                masked(&account.organization),
                account.plan,
            );
            println!("  usage: {:?}", account.usage);
            println!("  failure: {:?}", account.usage_failure);
        }
    }

    /// Runs the usage probe once against this device's default login and
    /// prints the reading.
    #[tokio::test]
    #[ignore = "runs this device's real claude"]
    async fn real_probe() {
        let claude = Cli::find().expect("claude on this device");
        let home = std::env::var_os("HOME").expect("HOME");
        let config = ConfigDir::default_in(Path::new(&home));
        let started = std::time::Instant::now();
        let usage = claude.probe(&config).await;
        println!("probe took {:?}: {usage:?}", started.elapsed());
        assert!(usage.is_ok());
    }
}
