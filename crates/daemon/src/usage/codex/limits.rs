// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Turns the rate limits Codex reports into usage windows. The app-server
//! names fields in camel case and rollouts in snake case, and both give a
//! limit a primary and a secondary window.

use maxhq_protocol::model::{Usage, UsagePeriod, UsageSource, UsageWindow};
use maxhq_protocol::time::Timestamp;
use serde_json::Value;

/// The limit every Codex plan has. Its windows are the account's own, so
/// they have no scope.
pub const OWN_LIMIT: &str = "codex";

/// The windows of one Codex limit.
#[derive(Clone, Debug, PartialEq)]
pub struct Limit {
    /// The limit's ID, such as `codex`.
    pub id: String,
    /// Its windows, or `None` when Codex sent none, which keeps the ones
    /// read before.
    pub windows: Option<Vec<UsageWindow>>,
}

/// One report of an account's limits.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    /// When Codex reported them.
    pub checked_at: Timestamp,
    /// Where they came from.
    pub source: UsageSource,
    /// The limits reported.
    pub limits: Vec<Limit>,
    /// Whether `limits` holds every limit the account has, so any other one
    /// shown is gone. The app-server reports them all, and a rollout only
    /// the one its session used.
    pub complete: bool,
}

/// The field names in one format.
struct Keys {
    name: &'static str,
    used: &'static str,
    minutes: &'static str,
    resets: &'static str,
}

/// The app-server's field names.
const APP_SERVER: Keys = Keys {
    name: "limitName",
    used: "usedPercent",
    minutes: "windowDurationMins",
    resets: "resetsAt",
};

/// The field names in rollouts.
const ROLLOUT: Keys = Keys {
    name: "limit_name",
    used: "used_percent",
    minutes: "window_minutes",
    resets: "resets_at",
};

/// The limits in an `account/rateLimits/read` result, read at `checked_at`.
/// An older app-server without `rateLimitsByLimitId` still sends
/// `rateLimits`.
pub fn from_app_server(result: &Value, checked_at: Timestamp) -> Reading {
    let limits = match result.get("rateLimitsByLimitId").and_then(Value::as_object) {
        Some(by_id) => by_id
            .iter()
            .map(|(id, snapshot)| limit(id, snapshot, &APP_SERVER))
            .collect(),
        None => result
            .get("rateLimits")
            .filter(|snapshot| snapshot.is_object())
            .map(|snapshot| {
                let id = snapshot
                    .get("limitId")
                    .and_then(Value::as_str)
                    .unwrap_or(OWN_LIMIT);
                vec![limit(id, snapshot, &APP_SERVER)]
            })
            .unwrap_or_default(),
    };
    Reading {
        checked_at,
        source: UsageSource::CodexAppServer,
        limits,
        complete: true,
    }
}

/// The limit in a `token_count` event's `rate_limits`, written at
/// `checked_at`.
pub fn from_rollout(rate_limits: &Value, checked_at: Timestamp) -> Option<Reading> {
    if !rate_limits.is_object() {
        return None;
    }
    let id = rate_limits
        .get("limit_id")
        .and_then(Value::as_str)
        .unwrap_or(OWN_LIMIT);
    Some(Reading {
        checked_at,
        source: UsageSource::CodexRollout,
        limits: vec![limit(id, rate_limits, &ROLLOUT)],
        complete: false,
    })
}

/// The usage to show once `reading` arrives, given what showed before. A
/// reading older than that changes nothing.
pub fn merge(earlier: Option<Usage>, reading: &Reading) -> Option<Usage> {
    if let Some(shown) = &earlier
        && shown.checked_at > reading.checked_at
    {
        return earlier;
    }
    let mut windows = earlier.map(|usage| usage.windows).unwrap_or_default();
    windows.retain(|window| {
        match reading
            .limits
            .iter()
            .find(|limit| scope(&limit.id) == window.scope)
        {
            // A limit that sent no windows keeps the ones it had.
            Some(limit) => limit.windows.is_none(),
            None => !reading.complete,
        }
    });
    for limit in &reading.limits {
        windows.extend(limit.windows.iter().flatten().cloned());
    }
    // No scope sorts first, so the account's own windows lead.
    windows.sort_by(|a, b| (&a.scope, a.window_minutes).cmp(&(&b.scope, b.window_minutes)));
    Some(Usage {
        checked_at: reading.checked_at,
        source: reading.source,
        windows,
    })
}

/// The scope of a limit's windows: none for the account's own limit, and
/// the limit's ID for any other.
pub fn scope(id: &str) -> Option<String> {
    (id != OWN_LIMIT).then(|| id.to_owned())
}

/// One limit's windows from a snapshot, each with the limit's name when
/// Codex gives one.
fn limit(id: &str, snapshot: &Value, keys: &Keys) -> Limit {
    let name = snapshot.get(keys.name).and_then(Value::as_str);
    let windows: Vec<UsageWindow> = ["primary", "secondary"]
        .into_iter()
        .filter_map(|which| window(snapshot.get(which)?, scope(id), name, keys))
        .collect();
    Limit {
        id: id.to_owned(),
        windows: (!windows.is_empty()).then_some(windows),
    }
}

/// One window, or None when it is null or lacks its length or use.
fn window(
    value: &Value,
    scope: Option<String>,
    name: Option<&str>,
    keys: &Keys,
) -> Option<UsageWindow> {
    let window_minutes = u32::try_from(value.get(keys.minutes)?.as_u64()?).ok()?;
    let used_percent = value.get(keys.used)?.as_f64()?;
    let resets_at = value
        .get(keys.resets)
        .and_then(Value::as_u64)
        .map(|secs| Timestamp(secs.saturating_mul(1000)));
    Some(UsageWindow {
        scope,
        name: name.map(str::to_owned),
        window_minutes,
        used_percent,
        resets_at,
        period: UsagePeriod::Rolling,
        spend: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const RATE_LIMITS: &str = include_str!("fixtures/rate_limits.json");

    fn window(scope: Option<&str>, minutes: u32, used: f64, resets: u64) -> UsageWindow {
        UsageWindow {
            scope: scope.map(str::to_owned),
            name: None,
            window_minutes: minutes,
            used_percent: used,
            resets_at: Some(Timestamp(resets * 1000)),
            period: UsagePeriod::Rolling,
            spend: None,
        }
    }

    fn app_server_reading() -> Reading {
        let result: Value = serde_json::from_str(RATE_LIMITS).unwrap();
        from_app_server(&result, Timestamp(1_000))
    }

    #[test]
    fn app_server_limits_become_windows() {
        let usage = merge(None, &app_server_reading()).unwrap();
        assert_eq!(usage.source, UsageSource::CodexAppServer);
        assert_eq!(usage.checked_at, Timestamp(1_000));
        assert_eq!(
            usage.windows,
            vec![
                window(None, 300, 41.0, 1_792_907_222),
                window(None, 10_080, 12.0, 1_793_494_022),
                UsageWindow {
                    name: Some("Codex Mini".into()),
                    ..window(Some("codex_mini"), 300, 3.0, 1_792_906_000)
                },
            ]
        );
    }

    #[test]
    fn a_rollout_carries_the_limit_name() {
        let rate_limits = json!({
            "limit_id": "codex_mini",
            "limit_name": "Codex Mini",
            "primary": { "used_percent": 8.0, "window_minutes": 300, "resets_at": 1_792_906_000 },
            "secondary": { "used_percent": 2.0, "window_minutes": 10080, "resets_at": 1_793_494_022 }
        });
        let reading = from_rollout(&rate_limits, Timestamp(1)).unwrap();
        let windows = reading.limits[0].windows.as_ref().unwrap();
        let names: Vec<_> = windows
            .iter()
            .map(|w| (w.scope.as_deref(), w.name.as_deref(), w.window_minutes))
            .collect();
        assert_eq!(
            names,
            [
                (Some("codex_mini"), Some("Codex Mini"), 300),
                (Some("codex_mini"), Some("Codex Mini"), 10_080)
            ]
        );
    }

    #[test]
    fn null_windows_keep_the_earlier_ones() {
        let earlier = Usage {
            checked_at: Timestamp(500),
            source: UsageSource::CodexAppServer,
            windows: vec![
                window(None, 300, 1.0, 1),
                window(Some("codex_premium"), 10_080, 77.0, 9),
                window(Some("codex_retired"), 10_080, 5.0, 9),
            ],
        };
        let usage = merge(Some(earlier), &app_server_reading()).unwrap();
        let scopes: Vec<_> = usage
            .windows
            .iter()
            .map(|w| (w.scope.as_deref(), w.window_minutes, w.used_percent))
            .collect();
        assert_eq!(
            scopes,
            vec![
                (None, 300, 41.0),
                (None, 10_080, 12.0),
                (Some("codex_mini"), 300, 3.0),
                // codex_premium came back with null windows, so it keeps
                // its last numbers. codex_retired is gone.
                (Some("codex_premium"), 10_080, 77.0),
            ]
        );
    }

    #[test]
    fn rollout_limits_update_only_their_own_limit_when_newer() {
        let shown = merge(None, &app_server_reading());
        let rate_limits = json!({
            "limit_id": "codex",
            "limit_name": null,
            "primary": { "used_percent": 55.5, "window_minutes": 300, "resets_at": 1_792_907_222 },
            "secondary": { "used_percent": 13.0, "window_minutes": 10080, "resets_at": 1_793_494_022 },
            "credits": null,
            "plan_type": "plus"
        });
        let newer = from_rollout(&rate_limits, Timestamp(2_000)).unwrap();
        let usage = merge(shown.clone(), &newer).unwrap();
        assert_eq!(usage.source, UsageSource::CodexRollout);
        assert_eq!(usage.windows[0].used_percent, 55.5);
        assert_eq!(usage.windows[1].used_percent, 13.0);
        assert_eq!(
            usage.windows[2].scope.as_deref(),
            Some("codex_mini"),
            "a rollout reports one limit, so the others stay"
        );

        let older = from_rollout(&rate_limits, Timestamp(10)).unwrap();
        assert_eq!(merge(shown.clone(), &older), shown);
    }

    #[test]
    fn a_missing_by_id_map_falls_back_to_the_main_limit() {
        let result = json!({
            "rateLimits": {
                "limitId": "codex",
                "primary": { "usedPercent": 99, "windowDurationMins": 300, "resetsAt": 1_792_907_222 },
                "secondary": null
            },
            "rateLimitsByLimitId": null
        });
        let usage = merge(None, &from_app_server(&result, Timestamp(1))).unwrap();
        assert_eq!(usage.windows, vec![window(None, 300, 99.0, 1_792_907_222)]);
    }

    #[test]
    fn a_window_without_a_length_is_skipped() {
        let rate_limits = json!({
            "primary": { "used_percent": 10.0, "window_minutes": null, "resets_at": null },
            "secondary": { "used_percent": 20.0, "window_minutes": 10080 }
        });
        let reading = from_rollout(&rate_limits, Timestamp(1)).unwrap();
        assert_eq!(reading.limits[0].id, OWN_LIMIT);
        let windows = reading.limits[0].windows.as_ref().unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].window_minutes, 10_080);
        assert_eq!(windows[0].resets_at, None);
    }
}
