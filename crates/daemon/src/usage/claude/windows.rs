// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Turns Claude's `rate_limits` object into usage windows. The probe's reply
//! and the cache in `.claude.json` use the same shape.

use maxhq_protocol::model::{UsagePeriod, UsageWindow};
use serde_json::Value;

use crate::observe::claude::time::parse_iso8601;

/// The 5-hour window, in minutes.
const FIVE_HOURS: u32 = 300;

/// The weekly window, in minutes.
const WEEK: u32 = 10_080;

/// The models whose weekly limit a `seven_day_<model>` key holds. Other keys
/// that start with `seven_day_` are internal code names or aren't per model,
/// so they're left out.
const MODELS: [&str; 4] = ["opus", "sonnet", "haiku", "fable"];

/// The windows in a `rate_limits` object: the 5-hour and weekly ones first,
/// then each model's weekly one. A key that's null, missing or has no number
/// for its percent gives no window. A model's weekly limit can also come in
/// the `limits` list, under the model's own name, and counts once.
pub fn windows(rate_limits: &Value) -> Vec<UsageWindow> {
    let own = [("five_hour", FIVE_HOURS), ("seven_day", WEEK)]
        .into_iter()
        .filter_map(|(key, minutes)| window(rate_limits.get(key)?, UTILIZATION, None, minutes));
    let models = MODELS.into_iter().filter_map(|model| {
        let limit = rate_limits.get(format!("seven_day_{model}"))?;
        window(limit, UTILIZATION, Some(model), WEEK)
    });
    let mut windows: Vec<UsageWindow> = own.chain(models).collect();
    for listed in listed_models(rate_limits) {
        if !windows.iter().any(|w| w.scope == listed.scope) {
            windows.push(listed);
        }
    }
    windows
}

/// Where a `seven_day` style limit keeps its percent.
const UTILIZATION: &str = "utilization";

/// The weekly limits in the `limits` list that cover one model, such as
/// `{"kind": "weekly_scoped", "percent": 0, "scope": {"model": {"id": null,
/// "display_name": "Fable"}}}`. The scope is the model's ID, or its name in
/// lower case when it has none, and the window carries the name.
fn listed_models(rate_limits: &Value) -> impl Iterator<Item = UsageWindow> + '_ {
    let list = rate_limits.get("limits").and_then(Value::as_array);
    list.into_iter().flatten().filter_map(|limit| {
        if limit.get("kind")?.as_str()? != "weekly_scoped" {
            return None;
        }
        let model = limit.get("scope")?.get("model")?;
        let name = model.get("display_name").and_then(Value::as_str);
        let scope = model.get("id").and_then(Value::as_str).or(name)?;
        let mut window = window(limit, "percent", Some(&scope.to_lowercase()), WEEK)?;
        window.name = name.map(str::to_owned);
        Some(window)
    })
}

/// One window from a limit such as `{"utilization": 22, "resets_at": ...}`,
/// with its percent under `used`.
fn window(
    limit: &Value,
    used: &str,
    scope: Option<&str>,
    window_minutes: u32,
) -> Option<UsageWindow> {
    let used = limit.get(used)?.as_f64()?;
    Some(UsageWindow {
        scope: scope.map(str::to_owned),
        name: None,
        window_minutes,
        used_percent: used.clamp(0.0, 100.0),
        resets_at: limit
            .get("resets_at")
            .and_then(Value::as_str)
            .and_then(parse_iso8601),
        period: UsagePeriod::Rolling,
        spend: None,
    })
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::time::Timestamp;
    use serde_json::json;

    use super::*;

    #[test]
    fn maps_the_account_windows_then_each_model() {
        let limits = json!({
            "five_hour": {"utilization": 22, "resets_at": "2026-10-25T02:29:59.941420+00:00", "limit_dollars": null},
            "seven_day": {"utilization": 45.5, "resets_at": "2026-10-29T01:59:59.941439+00:00"},
            "seven_day_oauth_apps": null,
            "seven_day_opus": {"utilization": 12, "resets_at": "2026-10-29T02:00:00+00:00"},
            "seven_day_sonnet": null,
            "seven_day_widget": {"utilization": 80, "resets_at": "2026-10-29T02:00:00+00:00"},
            "blue_heron": {"utilization": 3, "resets_at": "2026-11-05T07:59:00+00:00", "limit_dollars": 50},
            "extra_usage": {"is_enabled": false, "utilization": null}
        });
        let windows = windows(&limits);
        let summary: Vec<(Option<&str>, u32, f64)> = windows
            .iter()
            .map(|w| (w.scope.as_deref(), w.window_minutes, w.used_percent))
            .collect();
        assert_eq!(
            summary,
            [
                (None, 300, 22.0),
                (None, 10_080, 45.5),
                (Some("opus"), 10_080, 12.0)
            ]
        );
        assert_eq!(windows[0].resets_at, Some(Timestamp(1_792_895_399_941)));
        assert_eq!(windows[2].resets_at, parse_iso8601("2026-10-29T02:00:00Z"));
    }

    #[test]
    fn skips_windows_without_a_percent() {
        let limits = json!({
            "five_hour": {"utilization": null, "resets_at": "2026-10-25T02:29:59Z"},
            "seven_day": {"utilization": 101, "resets_at": null},
            "seven_day_fable": {"utilization": -1}
        });
        let windows = windows(&limits);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].window_minutes, 10_080);
        assert_eq!(windows[0].used_percent, 100.0);
        assert_eq!(windows[0].resets_at, None);
        assert_eq!(windows[1].scope.as_deref(), Some("fable"));
        assert_eq!(windows[1].used_percent, 0.0);
        assert!(super::windows(&json!(null)).is_empty());
    }

    #[test]
    fn reads_a_model_limit_from_the_list_with_its_name() {
        let limits = json!({
            "seven_day": {"utilization": 31, "resets_at": "2026-11-26T02:00:00+00:00"},
            "seven_day_opus": {"utilization": 40, "resets_at": "2026-11-26T02:00:00+00:00"},
            "limits": [
                {"kind": "weekly_all", "percent": 31, "resets_at": "2026-11-26T02:00:00+00:00", "scope": null},
                {"kind": "weekly_scoped", "percent": 18, "resets_at": "2026-11-26T02:00:00+00:00",
                 "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null}},
                {"kind": "weekly_scoped", "percent": 55,
                 "scope": {"model": {"id": "opus", "display_name": "Opus"}}},
                {"kind": "weekly_scoped", "percent": 9, "scope": {"model": {"id": null, "display_name": null}}}
            ]
        });
        let windows = windows(&limits);
        let summary: Vec<_> = windows
            .iter()
            .map(|w| (w.scope.as_deref(), w.name.as_deref(), w.used_percent))
            .collect();
        assert_eq!(
            summary,
            [
                (None, None, 31.0),
                // The `seven_day_opus` key came first, so the list's Opus
                // entry doesn't add a second one.
                (Some("opus"), None, 40.0),
                (Some("fable"), Some("Fable"), 18.0),
            ]
        );
        assert_eq!(windows[2].window_minutes, 10_080);
        assert_eq!(windows[2].resets_at, parse_iso8601("2026-11-26T02:00:00Z"));
    }
}
