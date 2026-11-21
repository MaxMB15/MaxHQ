// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The account's rate limits and models, as the CLI reports them on the
//! stream. A `rate_limit_event` carries every window in `unifiedWindows`,
//! with use as a fraction and resets in Unix seconds.

use maxhq_protocol::model::{ModelChoice, Usage, UsagePeriod, UsageSource, UsageWindow};
use maxhq_protocol::time::Timestamp;
use serde_json::Value;

use super::effort::{self, Saved};
use super::wire::{text, unix_seconds};

/// The models a weekly limit can cover on its own.
const MODELS: [&str; 4] = ["opus", "sonnet", "haiku", "fable"];

/// How long each window the CLI names lasts, in minutes.
fn window_minutes(name: &str) -> Option<(Option<String>, u32)> {
    match name {
        "five_hour" => Some((None, 300)),
        "seven_day" => Some((None, 10_080)),
        _ => {
            let model = name.strip_prefix("seven_day_")?;
            MODELS
                .contains(&model)
                .then(|| (Some(model.to_owned()), 10_080))
        }
    }
}

/// The account's usage from a `rate_limit_info`, or none when it names no
/// window `maxd` knows.
pub fn usage(info: &Value, at: Timestamp) -> Option<Usage> {
    let mut windows: Vec<UsageWindow> = info
        .get("unifiedWindows")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, window)| {
            let (scope, window_minutes) = window_minutes(name)?;
            let used = window.get("utilization").and_then(Value::as_f64)?;
            Some(UsageWindow {
                scope,
                name: None,
                window_minutes,
                used_percent: (used * 100.0).clamp(0.0, 100.0),
                resets_at: window.get("resetsAt").and_then(unix_seconds),
                period: UsagePeriod::Rolling,
                spend: None,
            })
        })
        .collect();
    if windows.is_empty() {
        return None;
    }
    // The account's own limits first, shortest window first.
    windows.sort_by(|a, b| {
        (a.scope.is_some(), a.window_minutes, &a.scope).cmp(&(
            b.scope.is_some(),
            b.window_minutes,
            &b.scope,
        ))
    });
    Some(Usage {
        checked_at: at,
        source: UsageSource::ClaudeProbe,
        windows,
    })
}

/// When the limit that stops the account resets, if one does.
pub fn rejected(info: &Value) -> Option<Timestamp> {
    if text(info, "status") != Some("rejected") {
        return None;
    }
    info.get("resetsAt").and_then(unix_seconds).or_else(|| {
        info.get("unifiedWindows")
            .and_then(Value::as_object)?
            .values()
            .filter(|window| text(window, "status") == Some("rejected"))
            .filter_map(|window| window.get("resetsAt").and_then(unix_seconds))
            .max()
    })
}

/// The models from an `initialize` reply. The CLI lists its default as a
/// model of its own, so that entry marks the alias it resolves to instead.
/// The CLI doesn't say which effort level each runs at when told none, so
/// that comes from what the login's settings in `saved` keep, and the CLI.
pub fn models(reply: &Value, saved: &Saved) -> Option<Vec<ModelChoice>> {
    let list = reply.get("models")?.as_array()?;
    let default = list
        .iter()
        .find(|model| text(model, "value") == Some("default"))
        .and_then(|model| text(model, "resolvedModel"));
    let mut picked = false;
    let choices = list
        .iter()
        .filter(|model| text(model, "value") != Some("default"))
        .filter_map(|model| {
            let id = text(model, "value")?;
            let is_default =
                !picked && default.is_some() && text(model, "resolvedModel") == default;
            picked |= is_default;
            let mut choice = ModelChoice {
                id: id.to_owned(),
                label: text(model, "displayName").unwrap_or(id).to_owned(),
                default: is_default,
                efforts: efforts(model),
                default_effort: None,
                no_images: false,
            };
            choice.default_effort =
                effort::default_for(&choice, text(model, "resolvedModel"), saved);
            Some(choice)
        })
        .collect();
    Some(choices)
}

/// The effort levels a model from an `initialize` reply takes, in the
/// CLI's order.
fn efforts(model: &Value) -> Vec<String> {
    if model.get("supportsEffort").and_then(Value::as_bool) != Some(true) {
        return Vec::new();
    }
    model
        .get("supportedEffortLevels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unified_windows_become_percentages() {
        let info = json!({"status": "allowed", "unifiedWindows": {
            "seven_day": {"utilization": 0.57, "resetsAt": 1_793_239_200, "status": "allowed"},
            "seven_day_opus": {"utilization": 0.2, "resetsAt": 1_793_239_200},
            "five_hour": {"utilization": 0.1, "resetsAt": 1_792_936_200, "status": "allowed"},
            "overage": {"utilization": 0.0},
        }});
        let usage = usage(&info, Timestamp(5)).unwrap();
        let shown: Vec<_> = usage
            .windows
            .iter()
            .map(|window| {
                (
                    window.scope.as_deref(),
                    window.window_minutes,
                    window.used_percent.round(),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                (None, 300, 10.0),
                (None, 10_080, 57.0),
                (Some("opus"), 10_080, 20.0)
            ]
        );
        assert_eq!(
            usage.windows[0].resets_at,
            Some(Timestamp(1_792_936_200_000))
        );
        assert_eq!(rejected(&info), None);
    }

    #[test]
    fn a_rejected_limit_gives_its_reset() {
        let info = json!({"status": "rejected", "resetsAt": 1_792_936_200});
        assert_eq!(rejected(&info), Some(Timestamp(1_792_936_200_000)));
        let info = json!({"status": "rejected", "unifiedWindows": {
            "five_hour": {"utilization": 1.0, "resetsAt": 1_792_936_200, "status": "rejected"}}});
        assert_eq!(rejected(&info), Some(Timestamp(1_792_936_200_000)));
    }

    #[test]
    fn the_default_marks_the_alias_it_resolves_to() {
        let reply = json!({"models": [
            {"value": "default", "resolvedModel": "claude-opus-5-5", "displayName": "Default (recommended)"},
            {"value": "opus", "resolvedModel": "claude-opus-5-5", "displayName": "Opus 5.5",
             "supportsEffort": true, "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"]},
            {"value": "sonnet", "resolvedModel": "claude-sonnet-5-5", "displayName": "Sonnet 5.5",
             "supportsEffort": false, "supportedEffortLevels": ["low"]},
        ]});
        assert_eq!(
            models(&reply, &Saved::default()).unwrap(),
            [
                ModelChoice {
                    id: "opus".into(),
                    label: "Opus 5.5".into(),
                    default: true,
                    efforts: ["low", "medium", "high", "xhigh", "max"]
                        .map(String::from)
                        .into(),
                    default_effort: Some("medium".into()),
                    no_images: false,
                },
                ModelChoice {
                    id: "sonnet".into(),
                    label: "Sonnet 5.5".into(),
                    default: false,
                    efforts: Vec::new(),
                    default_effort: None,
                    no_images: false,
                },
            ]
        );
    }
}
