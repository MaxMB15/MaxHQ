// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Usage for Codex logins, per decision C-6. Each login is a `CODEX_HOME`,
//! and `maxd` starts a `codex app-server` for it just long enough to read
//! the rate limits, unless an agent's app-server read them lately. Sessions
//! also write the limits into their rollouts, which the observer passes on
//! here when they're newer.

pub(crate) mod account;
mod app_server;
pub mod limits;
pub mod models;
pub(crate) mod reset;

/// The stand-in app-server, for tests that spend a reset elsewhere.
#[cfg(test)]
pub(crate) use app_server::tests::FakeHome;

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use maxhq_core::names;
use maxhq_protocol::ids::{AccountId, DeviceId};
use maxhq_protocol::model::Vendor;
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::hub::Hub;
use limits::Reading;

/// More `CODEX_HOME` folders to watch besides `~/.codex`, colon separated.
pub const HOMES_ENV: &str = "MAXD_CODEX_HOMES";

/// One `CODEX_HOME`, which holds one login.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Home {
    /// The folder as configured, with `~` expanded.
    pub dir: PathBuf,
    /// The same folder with links resolved, as `lsof` shows paths in it.
    pub real: PathBuf,
}

impl Home {
    /// The folder as the account entity names it.
    pub fn config_dir(&self) -> String {
        self.dir.to_string_lossy().into_owned()
    }

    /// The ID of the login this folder holds on `device`.
    pub fn account(&self, device: DeviceId) -> AccountId {
        names::account(device, Vendor::Codex, &self.config_dir())
    }

    /// Whether `folder` is this one, as configured or resolved.
    pub fn is(&self, folder: &Path) -> bool {
        folder == self.dir || folder == self.real
    }
}

/// The folders to watch on this device: `~/.codex`, then each folder in
/// [`HOMES_ENV`]. Folders that don't exist are left out.
pub fn homes() -> Vec<Home> {
    homes_from(
        std::env::home_dir().as_deref(),
        std::env::var_os(HOMES_ENV).as_deref(),
    )
}

/// [`homes`] for a given home folder and value of [`HOMES_ENV`]. Relative
/// paths are skipped, and a folder named twice counts once.
pub fn homes_from(user_home: Option<&Path>, extra: Option<&OsStr>) -> Vec<Home> {
    let default = user_home.map(|home| home.join(".codex"));
    let extra = extra
        .and_then(OsStr::to_str)
        .into_iter()
        .flat_map(|list| list.split(':'))
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| match entry.strip_prefix("~/") {
            Some(rest) => user_home.map(|home| home.join(rest)),
            None => Some(PathBuf::from(entry)),
        });
    let mut homes: Vec<Home> = Vec::new();
    for dir in default.into_iter().chain(extra) {
        if !dir.is_absolute() {
            continue;
        }
        // Drop a trailing slash, so the account ID doesn't depend on it.
        let dir: PathBuf = dir.components().collect();
        let Ok(real) = dir.canonicalize() else {
            continue;
        };
        if real.is_dir() && !homes.iter().any(|home| home.real == real) {
            homes.push(Home { dir, real });
        }
    }
    homes
}

/// The newest rate limits a rollout showed for each account, from the
/// observer.
static ROLLOUTS: LazyLock<watch::Sender<HashMap<AccountId, Reading>>> =
    LazyLock::new(|| watch::Sender::new(HashMap::new()));

/// Passes on the rate limits a session wrote to its rollout. Only a reading
/// newer than the last one for the account counts.
pub fn report_rollout(account: AccountId, reading: Reading) {
    ROLLOUTS.send_if_modified(|all| {
        let newer = all
            .get(&account)
            .is_none_or(|known| known.checked_at < reading.checked_at);
        if newer {
            all.insert(account, reading);
        }
        newer
    });
}

/// The newest rollout reading passed on for `account`.
#[cfg(test)]
pub fn rollout_reading(account: AccountId) -> Option<Reading> {
    ROLLOUTS.borrow().get(&account).cloned()
}

/// Keeps this vendor's account entities current until the process exits.
pub async fn run(hub: Hub) {
    let homes = homes();
    let mut tasks = JoinSet::new();
    for home in homes {
        tasks.spawn(account::watch(hub.clone(), home));
    }
    while tasks.join_next().await.is_some() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("maxd-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn homes_are_the_default_and_the_listed_folders() {
        let temp = TempDir::new("codex-homes");
        let user = temp.0.join("user");
        for dir in [".codex", "work-codex", "spare"] {
            std::fs::create_dir_all(user.join(dir)).unwrap();
        }
        let spare = user.join("spare");
        let list = format!(
            "~/work-codex: {}/ :relative/codex:/no/such/folder::{}",
            spare.display(),
            user.join(".codex").display()
        );
        let homes = homes_from(Some(&user), Some(OsStr::new(&list)));
        let dirs: Vec<&Path> = homes.iter().map(|home| home.dir.as_path()).collect();
        assert_eq!(
            dirs,
            [user.join(".codex"), user.join("work-codex"), spare.clone()]
        );
        assert!(homes[2].is(&spare.canonicalize().unwrap()));
        assert_eq!(homes[2].config_dir(), spare.to_string_lossy());
    }

    #[test]
    fn a_missing_default_home_is_left_out() {
        let temp = TempDir::new("codex-no-home");
        assert!(homes_from(Some(&temp.0), None).is_empty());
        assert!(homes_from(None, Some(OsStr::new("~/x"))).is_empty());
    }

    #[test]
    fn only_newer_rollout_readings_are_passed_on() {
        use maxhq_protocol::model::UsageSource;
        use maxhq_protocol::time::Timestamp;

        let account = AccountId::derive("only-newer-rollout-readings");
        let reading = |at| Reading {
            checked_at: Timestamp(at),
            source: UsageSource::CodexRollout,
            limits: Vec::new(),
            complete: false,
            reset_credits: None,
        };
        let mut seen = ROLLOUTS.subscribe();
        report_rollout(account, reading(20));
        report_rollout(account, reading(10));
        assert_eq!(seen.borrow_and_update()[&account].checked_at, Timestamp(20));
        report_rollout(account, reading(30));
        assert!(seen.has_changed().unwrap());
        assert_eq!(seen.borrow_and_update()[&account].checked_at, Timestamp(30));
    }
}
