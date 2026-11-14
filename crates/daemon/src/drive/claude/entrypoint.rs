// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The name a Claude process MaxHQ starts gives for the app that runs it.
//! Claude Code reads it from `CLAUDE_CODE_ENTRYPOINT`, and only from its
//! process environment, never from its settings `env`. So a name a
//! login's settings give is read here and passed on.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The variable Claude Code reads the name from.
pub const VAR: &str = "CLAUDE_CODE_ENTRYPOINT";

/// MaxHQ's own name, for every process whose settings give no other.
pub const MAXHQ: &str = "maxhq";

/// The settings an administrator sets for every login on this device.
#[cfg(target_os = "macos")]
const MANAGED: &str = "/Library/Application Support/ClaudeCode/managed-settings.json";
#[cfg(not(target_os = "macos"))]
const MANAGED: &str = "/etc/claude-code/managed-settings.json";

/// The name for a Claude process under the login in `config_dir`: the one
/// its settings `env` gives, or [`MAXHQ`].
pub fn label(config_dir: &Path) -> String {
    from_settings(&settings_files(config_dir)).unwrap_or_else(|| MAXHQ.to_owned())
}

/// The settings files of the login in `config_dir`: the user's own, then
/// the ones an administrator manages, which win.
pub(super) fn settings_files(config_dir: &Path) -> [PathBuf; 2] {
    [config_dir.join("settings.json"), PathBuf::from(MANAGED)]
}

/// The name the settings `env` in `files` gives, the last file that gives
/// one winning. Settings `env` may hold keys and tokens, so this reads
/// [`VAR`] and nothing else, and says nothing about a file it can't read
/// or parse. An empty name is none.
fn from_settings(files: &[PathBuf]) -> Option<String> {
    #[derive(Deserialize)]
    struct Settings {
        env: Option<Env>,
    }
    #[derive(Deserialize)]
    struct Env {
        #[serde(rename = "CLAUDE_CODE_ENTRYPOINT")]
        entrypoint: Option<String>,
    }
    files
        .iter()
        .rev()
        .find_map(|file| {
            let settings: Settings = serde_json::from_slice(&fs::read(file).ok()?).ok()?;
            settings.env?.entrypoint
        })
        // A NUL would stop the process from starting at all.
        .filter(|name| !name.is_empty() && !name.contains('\0'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    /// A user and a managed settings file in `dir`, written when given.
    fn files(dir: &TempDir, user: Option<&str>, managed: Option<&str>) -> [PathBuf; 2] {
        let paths = [
            dir.path().join("settings.json"),
            dir.path().join("managed-settings.json"),
        ];
        for (path, text) in paths.iter().zip([user, managed]) {
            if let Some(text) = text {
                fs::write(path, text).unwrap();
            }
        }
        paths
    }

    #[test]
    fn a_login_without_settings_says_maxhq() {
        let dir = TempDir::new();
        assert_eq!(from_settings(&files(&dir, None, None)), None);
        // `label` also reads this device's managed settings, which name
        // none on a machine that runs these tests.
        assert_eq!(label(dir.path()), MAXHQ);
    }

    #[test]
    fn the_logins_settings_name_it() {
        let dir = TempDir::new();
        let user = r#"{"env": {"CLAUDE_CODE_ENTRYPOINT": "custom"}}"#;
        assert_eq!(
            from_settings(&files(&dir, Some(user), None)).as_deref(),
            Some("custom")
        );
        assert_eq!(label(dir.path()), "custom");
    }

    #[test]
    fn managed_settings_win_over_the_users() {
        let dir = TempDir::new();
        let user = r#"{"env": {"CLAUDE_CODE_ENTRYPOINT": "custom"}}"#;
        let managed = r#"{"env": {"CLAUDE_CODE_ENTRYPOINT": "managed"}}"#;
        assert_eq!(
            from_settings(&files(&dir, Some(user), Some(managed))).as_deref(),
            Some("managed")
        );
        // Managed settings that name nothing leave the user's name.
        let silent = r#"{"env": {"OTHER": "1"}}"#;
        assert_eq!(
            from_settings(&files(&dir, Some(user), Some(silent))).as_deref(),
            Some("custom")
        );
    }

    #[test]
    fn other_keys_and_broken_files_name_nothing() {
        let dir = TempDir::new();
        let others = r#"{
            "model": "opus",
            "env": {"ANTHROPIC_BASE_URL": "https://example.invalid", "OTHER_KEY": "placeholder"}
        }"#;
        assert_eq!(from_settings(&files(&dir, Some(others), None)), None);

        for broken in [
            "{not json",
            r#"{"env": "placeholder"}"#,
            r#"{"env": {"CLAUDE_CODE_ENTRYPOINT": 7}}"#,
            r#"{"env": {"CLAUDE_CODE_ENTRYPOINT": ""}}"#,
            r#"{"env": {"CLAUDE_CODE_ENTRYPOINT": "a\u0000b"}}"#,
            r#"{"env": null}"#,
            "",
        ] {
            let dir = TempDir::new();
            assert_eq!(
                from_settings(&files(&dir, Some(broken), None)),
                None,
                "{broken}"
            );
        }

        // A broken managed file leaves the user's name.
        let dir = TempDir::new();
        let user = r#"{"env": {"CLAUDE_CODE_ENTRYPOINT": "custom"}}"#;
        assert_eq!(
            from_settings(&files(&dir, Some(user), Some("{not json"))).as_deref(),
            Some("custom")
        );
    }
}
