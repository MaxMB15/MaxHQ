// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The Claude Code config folders `maxd` watches. Each one holds one login,
//! so each one is an account.

use std::path::{Path, PathBuf};

use maxhq_core::names;
use maxhq_protocol::ids::{AccountId, DeviceId};
use maxhq_protocol::model::Vendor;

/// Names more config folders to watch beside `~/.claude`, colon separated.
pub const DIRS_ENV: &str = "MAXD_CLAUDE_CONFIG_DIRS";

/// One Claude Code config folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigDir {
    /// The folder, as an absolute path.
    pub path: PathBuf,
    /// The file where Claude Code keeps this login's own state, such as its
    /// last usage reading. It's `~/.claude.json` for `~/.claude` and
    /// `.claude.json` inside any other folder.
    pub state_file: PathBuf,
    /// Whether this is `~/.claude`, which Claude Code uses when
    /// `CLAUDE_CONFIG_DIR` is unset.
    pub default: bool,
}

impl ConfigDir {
    /// `~/.claude` for the home folder `home`.
    pub fn default_in(home: &Path) -> Self {
        Self {
            path: home.join(".claude"),
            state_file: home.join(".claude.json"),
            default: true,
        }
    }

    /// A folder that `CLAUDE_CONFIG_DIR` selects.
    pub fn custom(path: PathBuf) -> Self {
        Self {
            state_file: path.join(".claude.json"),
            path,
            default: false,
        }
    }

    /// The account for this login on `device`.
    pub fn account(&self, device: DeviceId) -> AccountId {
        names::account(device, Vendor::Claude, &self.path.to_string_lossy())
    }

    /// The `CLAUDE_CONFIG_DIR` that makes `claude` use this folder, or none
    /// for the default, which `claude` finds by itself.
    pub fn env_value(&self) -> Option<&Path> {
        (!self.default).then_some(self.path.as_path())
    }
}

/// The config folders on this device that exist: `~/.claude` and every one
/// in `MAXD_CLAUDE_CONFIG_DIRS`.
pub fn config_dirs() -> Vec<ConfigDir> {
    let Some(home) = maxhq_os::home::home_dir() else {
        tracing::warn!("there's no home folder, so there's no Claude config folder to watch");
        return Vec::new();
    };
    let extra = std::env::var(DIRS_ENV).ok();
    candidates(&home, extra.as_deref())
        .into_iter()
        .filter(|dir| dir.path.is_dir())
        .collect()
}

/// The folders `home` and a `MAXD_CLAUDE_CONFIG_DIRS` value name, in order
/// and without repeats, whether they exist or not. A leading `~` is the home
/// folder, and relative paths are skipped.
pub fn candidates(home: &Path, extra: Option<&str>) -> Vec<ConfigDir> {
    let mut dirs = vec![ConfigDir::default_in(home)];
    for entry in extra.unwrap_or_default().split(':').map(str::trim) {
        if entry.is_empty() {
            continue;
        }
        let path = match entry.strip_prefix('~') {
            Some("") => home.to_path_buf(),
            Some(rest) if rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
            _ => PathBuf::from(entry),
        };
        if !path.is_absolute() {
            tracing::warn!(entry, "skipping a relative path in {DIRS_ENV}");
            continue;
        }
        // Collecting the components drops a trailing slash, so `/a/` and
        // `/a` are one folder and one account.
        let path: PathBuf = path.components().collect();
        if dirs.iter().all(|dir| dir.path != path) {
            dirs.push(ConfigDir::custom(path));
        }
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn the_default_folder_keeps_its_state_beside_it() {
        let dir = ConfigDir::default_in(Path::new("/home/dev"));
        assert_eq!(dir.path, Path::new("/home/dev/.claude"));
        assert_eq!(dir.state_file, Path::new("/home/dev/.claude.json"));
        assert_eq!(dir.env_value(), None);

        let other = ConfigDir::custom(PathBuf::from("/home/dev/.claude-work"));
        assert_eq!(
            other.state_file,
            Path::new("/home/dev/.claude-work/.claude.json")
        );
        assert_eq!(other.env_value(), Some(Path::new("/home/dev/.claude-work")));
    }

    #[test]
    fn extra_folders_come_after_the_default_once_each() {
        let home = Path::new("/home/dev");
        let dirs = candidates(
            home,
            Some("~/.claude-work: /srv/claude/ ::relative/dir:~/.claude:/srv/claude"),
        );
        let paths: Vec<&Path> = dirs.iter().map(|dir| dir.path.as_path()).collect();
        assert_eq!(
            paths,
            [
                Path::new("/home/dev/.claude"),
                Path::new("/home/dev/.claude-work"),
                Path::new("/srv/claude"),
            ]
        );
        assert!(dirs[0].default);
        assert!(!dirs[1].default && !dirs[2].default);
        assert_eq!(candidates(home, None).len(), 1);
    }

    #[test]
    fn each_folder_is_its_own_account() {
        let device = DeviceId(Uuid::from_u128(7));
        let home = ConfigDir::default_in(Path::new("/home/dev"));
        let work = ConfigDir::custom(PathBuf::from("/home/dev/.claude-work"));
        assert_ne!(home.account(device), work.account(device));
        assert_eq!(
            home.account(device),
            names::account(device, Vendor::Claude, "/home/dev/.claude")
        );
    }
}
