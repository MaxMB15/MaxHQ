// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The effort level a Claude model runs at when told none. The CLI
//! doesn't report it, so it's worked out the way the CLI does: the level
//! the login's settings save for the model, else the one the CLI ships
//! for it, else high. `maxd` then starts every agent at a level it names,
//! so the level the apps show is the one that runs.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use maxhq_protocol::model::ModelChoice;
use serde::Deserialize;
use serde_json::Value;

use super::entrypoint;

/// The level the CLI runs a model at when nothing else names one.
const FALLBACK: &str = "high";

/// The models the CLI ships with a level other than [`FALLBACK`].
const SHIPPED: [(&str, &str); 6] = [
    ("claude-haiku-5-5", "medium"),
    ("claude-sonnet-5-5", "medium"),
    ("claude-opus-4-7", "xhigh"),
    ("claude-opus-5-5", "medium"),
    ("claude-fable-5-2", "medium"),
    ("claude-mythos-5-2", "medium"),
];

/// The models a level the user saved for every model still applies to.
/// The CLI saves one per model now, and newer models ignore the old one.
const LEGACY: [&str; 19] = [
    "claude-3-5-haiku",
    "claude-3-5-sonnet",
    "claude-3-7-sonnet",
    "claude-haiku-4-5",
    "claude-sonnet-4-0",
    "claude-sonnet-4-5",
    "claude-sonnet-4-6",
    "claude-sonnet-5",
    "claude-opus-4-0",
    "claude-opus-4-1",
    "claude-opus-4-5",
    "claude-opus-4-6",
    "claude-opus-4-7",
    "claude-opus-4-8",
    "claude-opus-5",
    "claude-fable-5",
    "claude-fable-5-1",
    "claude-mythos-5",
    "claude-mythos-5-1",
];

/// The levels one settings file saves.
#[derive(Debug, Default, PartialEq, Eq)]
struct Levels {
    /// For every model.
    all: Option<String>,
    /// For each model, by its canonical name.
    by_model: HashMap<String, String>,
}

/// The levels a login's settings save: the user's own, and the ones an
/// administrator manages for every login on the device, which win.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Saved {
    user: Levels,
    managed: Levels,
}

impl Saved {
    /// What the settings of the login in `config_dir` save. A file that
    /// can't be read or parsed saves nothing.
    pub fn read(config_dir: &Path) -> Self {
        let [user, managed] = entrypoint::settings_files(config_dir);
        Self::from_files(&user, &managed)
    }

    fn from_files(user: &PathBuf, managed: &PathBuf) -> Self {
        Self {
            user: levels(user),
            managed: levels(managed),
        }
    }

    /// The level saved for `model`. Managed settings win, and a level
    /// they save for every model wins over the user's for one. The user's
    /// level for every model only reaches the older models that took it.
    fn level(&self, model: &str) -> Option<&str> {
        let model = canonical(model);
        self.managed
            .by_model
            .get(&model)
            .or(self.managed.all.as_ref())
            .or_else(|| self.user.by_model.get(&model))
            .or_else(|| {
                self.user
                    .all
                    .as_ref()
                    .filter(|_| LEGACY.contains(&model.as_str()) || !model.starts_with("claude-"))
            })
            .map(String::as_str)
    }
}

/// The level `choice` runs at when told none. `resolved` is the full model
/// name its alias stands for, when the CLI says. None when it lists no
/// levels.
pub fn default_for(choice: &ModelChoice, resolved: Option<&str>, saved: &Saved) -> Option<String> {
    let model = canonical(resolved.unwrap_or(&choice.id));
    let shipped = SHIPPED
        .iter()
        .find(|(name, _)| *name == model)
        .map_or(FALLBACK, |(_, level)| level);
    saved
        .level(&model)
        .and_then(|level| choice.fit_effort(level))
        .or_else(|| choice.fit_effort(shipped))
        .or_else(|| middle(&choice.efforts))
}

/// The middle of `efforts`, for a model that lists none of the others.
fn middle(efforts: &[String]) -> Option<String> {
    efforts.get(efforts.len() / 2).cloned()
}

/// A model's name as settings key it: lower case, without a provider
/// prefix, a date or a context size such as `[1m]`.
fn canonical(model: &str) -> String {
    let model = model.trim().to_lowercase();
    let model = model.rsplit('/').next().unwrap_or_default();
    let model = model.split('[').next().unwrap_or_default();
    match model.rsplit_once('-') {
        Some((base, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => {
            base.to_owned()
        }
        _ => model.to_owned(),
    }
}

/// The levels the settings file at `path` saves. Only the levels the CLI
/// saves count: it keeps "max" for one session.
fn levels(path: &PathBuf) -> Levels {
    #[derive(Deserialize)]
    struct Settings {
        #[serde(rename = "effortLevel")]
        effort_level: Option<Value>,
        #[serde(rename = "modelSettings")]
        model_settings: Option<HashMap<String, Value>>,
    }
    let level = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .filter(|level| matches!(*level, "low" | "medium" | "high" | "xhigh"))
            .map(str::to_owned)
    };
    let Some(settings) = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok())
    else {
        return Levels::default();
    };
    let mut by_model = HashMap::new();
    for (model, one) in settings.model_settings.unwrap_or_default() {
        let Some(found) = level(one.get("effortLevel")) else {
            continue;
        };
        // A key in canonical form wins over another spelling of it.
        let key = canonical(&model);
        if key == model || !by_model.contains_key(&key) {
            by_model.insert(key, found);
        }
    }
    Levels {
        all: level(settings.effort_level.as_ref()),
        by_model,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn opus(efforts: &[&str]) -> ModelChoice {
        ModelChoice {
            id: "opus".into(),
            label: "Opus".into(),
            default: true,
            efforts: efforts.iter().copied().map(String::from).collect(),
            default_effort: None,
            no_images: false,
        }
    }

    const ALL: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

    /// What a user and a managed settings file in `dir` save, each written
    /// when given.
    fn saved(dir: &TempDir, user: Option<&str>, managed: Option<&str>) -> Saved {
        let paths = [
            dir.path().join("settings.json"),
            dir.path().join("managed-settings.json"),
        ];
        for (path, text) in paths.iter().zip([user, managed]) {
            if let Some(text) = text {
                fs::write(path, text).unwrap();
            }
        }
        Saved::from_files(&paths[0], &paths[1])
    }

    #[test]
    fn without_settings_a_model_runs_at_what_the_cli_ships() {
        let none = Saved::default();
        let choice = opus(&ALL);
        assert_eq!(
            default_for(&choice, Some("claude-opus-5-5"), &none).as_deref(),
            Some("medium")
        );
        assert_eq!(
            default_for(&choice, Some("claude-opus-4-7"), &none).as_deref(),
            Some("xhigh")
        );
        // Any other model runs at high, and the alias stands in for a
        // model the CLI doesn't resolve.
        assert_eq!(
            default_for(&choice, Some("claude-opus-5"), &none).as_deref(),
            Some("high")
        );
        assert_eq!(default_for(&choice, None, &none).as_deref(), Some("high"));
    }

    #[test]
    fn a_model_without_the_level_takes_one_it_lists() {
        let none = Saved::default();
        // The CLI brings xhigh down to high on a model without it.
        assert_eq!(
            default_for(
                &opus(&["low", "medium", "high"]),
                Some("claude-opus-4-7"),
                &none
            )
            .as_deref(),
            Some("high")
        );
        // A model with none of the usual levels runs at its middle one.
        assert_eq!(
            default_for(&opus(&["slow", "steady", "fast"]), None, &none).as_deref(),
            Some("steady")
        );
        assert_eq!(default_for(&opus(&[]), None, &none), None);
    }

    #[test]
    fn the_users_level_for_the_model_wins() {
        let dir = TempDir::new();
        let user = r#"{"modelSettings": {"claude-opus-5-5": {"effortLevel": "xhigh"}}}"#;
        let saved = saved(&dir, Some(user), None);
        let choice = opus(&ALL);
        assert_eq!(
            default_for(&choice, Some("claude-opus-5-5[1m]"), &saved).as_deref(),
            Some("xhigh")
        );
        assert_eq!(
            default_for(&choice, Some("claude-opus-5-5-20261001"), &saved).as_deref(),
            Some("xhigh")
        );
        // Another model keeps what the CLI ships.
        assert_eq!(
            default_for(&choice, Some("claude-sonnet-5-5"), &saved).as_deref(),
            Some("medium")
        );
    }

    #[test]
    fn the_users_level_for_every_model_reaches_older_models_only() {
        let dir = TempDir::new();
        let saved = saved(&dir, Some(r#"{"effortLevel": "low"}"#), None);
        let choice = opus(&ALL);
        assert_eq!(
            default_for(&choice, Some("claude-opus-5"), &saved).as_deref(),
            Some("low")
        );
        assert_eq!(
            default_for(&choice, Some("claude-opus-5-5"), &saved).as_deref(),
            Some("medium")
        );
    }

    #[test]
    fn managed_settings_win() {
        let dir = TempDir::new();
        let user = r#"{"modelSettings": {"claude-opus-5-5": {"effortLevel": "xhigh"}}}"#;
        let managed = r#"{"effortLevel": "low"}"#;
        let both = saved(&dir, Some(user), Some(managed));
        assert_eq!(
            default_for(&opus(&ALL), Some("claude-opus-5-5"), &both).as_deref(),
            Some("low")
        );
        let dir = TempDir::new();
        let managed = r#"{"effortLevel": "low", "modelSettings": {"claude-opus-5-5": {"effortLevel": "high"}}}"#;
        let both = saved(&dir, Some(user), Some(managed));
        assert_eq!(
            default_for(&opus(&ALL), Some("claude-opus-5-5"), &both).as_deref(),
            Some("high")
        );
    }

    #[test]
    fn broken_settings_and_session_levels_save_nothing() {
        for broken in [
            "{not json",
            "",
            r#"{"effortLevel": 3}"#,
            r#"{"effortLevel": "max"}"#,
            r#"{"modelSettings": "opus"}"#,
            r#"{"modelSettings": {"claude-opus-5": {"effortLevel": "max"}}}"#,
        ] {
            let dir = TempDir::new();
            assert_eq!(
                saved(&dir, Some(broken), None),
                Saved::default(),
                "{broken}"
            );
        }
    }

    #[test]
    fn model_names_lose_their_date_size_and_provider() {
        assert_eq!(canonical("claude-opus-5-5"), "claude-opus-5-5");
        assert_eq!(canonical("Claude-Opus-5-5[1m]"), "claude-opus-5-5");
        assert_eq!(canonical("claude-sonnet-5-5-20261001"), "claude-sonnet-5-5");
        assert_eq!(canonical("vendor/claude-haiku-5-5"), "claude-haiku-5-5");
    }
}
