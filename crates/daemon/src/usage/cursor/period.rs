// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the
// GetCurrentPeriodUsage request with its headers, the response's
// billingCycleEnd and planUsage percentages, clamped and skipped when
// absent, and a failure that names neither the login nor what Cursor sent.
// The rest is MaxHQ's: the windows' shape and the on-demand spend.

//! Cursor's usage for the current billing cycle, from the dashboard
//! endpoint the CLI reads. Cursor doesn't document it, so a change on its
//! side shows as a failed check.

use std::time::Duration;

use maxhq_protocol::model::{Usage, UsagePeriod, UsageSource, UsageSpend, UsageWindow};
use maxhq_protocol::time::Timestamp;
use reqwest::header::CONTENT_TYPE;
use reqwest::{Client, StatusCode};
use serde_json::Value;

/// Where the CLI asks for the current period.
pub const ENDPOINT: &str =
    "https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage";

/// How long the call may take.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The plan's meters, each with the part of the plan it covers.
const METERS: [(&str, Option<&str>); 3] = [
    ("totalPercentUsed", None),
    ("autoPercentUsed", Some("auto")),
    ("apiPercentUsed", Some("api")),
];

/// The scope on-demand spend goes by.
pub const ON_DEMAND: &str = "on_demand";

/// Why asking Cursor failed. Neither names the login or what Cursor sent.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// Cursor turned the login down, so a fresh one may work.
    Rejected,
    /// Anything else, said in a few words.
    Other(String),
}

/// Asks `url` for the current period as the CLI does, with `token` as
/// the bearer.
pub async fn fetch(client: &Client, url: &str, token: &str) -> Result<Value, Failure> {
    let response = client
        .post(url)
        .bearer_auth(token)
        .header("connect-protocol-version", "1")
        .header("x-cursor-client-type", "cli")
        .header(CONTENT_TYPE, "application/json")
        .body("{}")
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|err| {
            Failure::Other(if err.is_timeout() {
                "Cursor took too long to answer".into()
            } else {
                "couldn't reach Cursor".into()
            })
        })?;
    match response.status() {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => return Err(Failure::Rejected),
        status if !status.is_success() => {
            return Err(Failure::Other(format!(
                "Cursor answered {}",
                status.as_u16()
            )));
        }
        _ => {}
    }
    let body = response
        .bytes()
        .await
        .map_err(|_| Failure::Other("Cursor's answer broke off".into()))?;
    serde_json::from_slice(&body).map_err(|_| Failure::Other("Cursor's answer isn't JSON".into()))
}

/// The windows Cursor's answer reports, or none when it has no meters.
/// Each window resets when the billing cycle ends.
pub fn usage(body: &Value, checked_at: Timestamp) -> Option<Usage> {
    let resets_at = number(&body["billingCycleEnd"])
        .filter(|end| *end > 0.0)
        .map(|end| Timestamp(end as u64));
    let window = |scope: Option<&str>, used_percent: f64, spend| UsageWindow {
        scope: scope.map(str::to_owned),
        name: None,
        window_minutes: 0,
        period: UsagePeriod::BillingCycle,
        used_percent: used_percent.clamp(0.0, 100.0),
        resets_at,
        spend,
    };
    let plan = &body["planUsage"];
    let mut windows: Vec<UsageWindow> = METERS
        .iter()
        .filter_map(|(field, scope)| Some(window(*scope, number(&plan[field])?, None)))
        .collect();
    if windows.is_empty() {
        return None;
    }
    if let Some(spend) = on_demand(&body["spendLimitUsage"]) {
        let percent = spend.used_cents as f64 * 100.0 / spend.limit_cents as f64;
        windows.push(window(Some(ON_DEMAND), percent, Some(spend)));
    }
    Some(Usage {
        checked_at,
        source: UsageSource::CursorDashboard,
        windows,
        reset_credits: None,
    })
}

/// On-demand spend against your own limit, or the team's when you have
/// none. Unset without a limit.
fn on_demand(spend: &Value) -> Option<UsageSpend> {
    ["individual", "pooled"].iter().find_map(|whose| {
        let cents = |what| number(&spend[format!("{whose}{what}")]).filter(|c| *c >= 0.0);
        let limit = cents("Limit").filter(|limit| *limit > 0.0)?;
        Some(UsageSpend {
            used_cents: cents("Used").unwrap_or(0.0).round() as u32,
            limit_cents: limit.round() as u32,
        })
    })
}

/// A finite number, which Cursor's JSON sends as a number or, for 64-bit
/// fields, a string.
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
    .filter(|number: &f64| number.is_finite())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use serde_json::json;

    use super::*;
    use crate::secrets::http;

    const CHECKED: Timestamp = Timestamp(1_789_000_000_000);

    /// A recorded answer, with the spend fields Cursor adds.
    fn answer() -> Value {
        json!({
            "billingCycleStart": "1787284386000",
            "billingCycleEnd": "1789876386000",
            "planUsage": {
                "totalPercentUsed": 72.4,
                "autoPercentUsed": 69.5,
                "apiPercentUsed": 100
            },
            "spendLimitUsage": {
                "individualLimit": 5000,
                "individualUsed": 1240
            },
            "enabled": true
        })
    }

    fn scopes(usage: &Usage) -> Vec<Option<&str>> {
        usage.windows.iter().map(|w| w.scope.as_deref()).collect()
    }

    #[test]
    fn the_answer_gives_month_meters_that_reset_with_the_cycle() {
        let usage = usage(&answer(), CHECKED).unwrap();
        assert_eq!(usage.source, UsageSource::CursorDashboard);
        assert_eq!(usage.checked_at, CHECKED);
        assert_eq!(
            scopes(&usage),
            [None, Some("auto"), Some("api"), Some(ON_DEMAND)]
        );
        let percents: Vec<f64> = usage.windows.iter().map(|w| w.used_percent).collect();
        assert_eq!(percents, [72.4, 69.5, 100.0, 24.8]);
        for window in &usage.windows {
            assert_eq!(window.period, UsagePeriod::BillingCycle);
            // 2026-09-20T03:53:06Z.
            assert_eq!(window.resets_at, Some(Timestamp(1_789_876_386_000)));
        }
        assert_eq!(usage.windows[0].spend, None);
        assert_eq!(
            usage.windows[3].spend,
            Some(UsageSpend {
                used_cents: 1240,
                limit_cents: 5000
            })
        );
    }

    #[test]
    fn absent_meters_are_left_out_and_none_means_no_usage() {
        assert_eq!(usage(&json!({ "planUsage": {} }), CHECKED), None);
        assert_eq!(usage(&json!({}), CHECKED), None);
        let only = usage(&json!({ "planUsage": { "totalPercentUsed": 0 } }), CHECKED).unwrap();
        assert_eq!(scopes(&only), [None]);
        assert_eq!(only.windows[0].used_percent, 0.0);
        assert_eq!(only.windows[0].resets_at, None, "no cycle end, no reset");
    }

    #[test]
    fn percents_are_clamped() {
        let over = json!({ "planUsage": { "totalPercentUsed": 150, "autoPercentUsed": -3 } });
        let usage = usage(&over, CHECKED).unwrap();
        assert_eq!(usage.windows[0].used_percent, 100.0);
        assert_eq!(usage.windows[1].used_percent, 0.0);
    }

    #[test]
    fn spend_needs_a_limit_and_falls_back_to_the_team() {
        let with = |spend: Value| {
            let mut body = answer();
            body["spendLimitUsage"] = spend;
            usage(&body, CHECKED).unwrap()
        };
        assert_eq!(scopes(&with(json!({}))).len(), 3);
        assert_eq!(scopes(&with(json!({ "individualLimit": 0 }))).len(), 3);
        let team = with(json!({ "pooledLimit": "20000", "pooledUsed": "5000" }));
        let spend = team.windows[3].spend.unwrap();
        assert_eq!((spend.used_cents, spend.limit_cents), (5000, 20000));
        assert_eq!(team.windows[3].used_percent, 25.0);
        let over = with(json!({ "individualLimit": 1000, "individualUsed": 1500 }));
        assert_eq!(over.windows[3].used_percent, 100.0);
    }

    /// A server on 127.0.0.1 that answers one request with `status` and
    /// `body`, and hands back the request it got.
    fn serve(status: &str, body: &'static str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/aiserver.v1.DashboardService/GetCurrentPeriodUsage",
            listener.local_addr().unwrap()
        );
        let status = status.to_owned();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut got = Vec::new();
            let mut chunk = [0; 4096];
            // The request is small and ends with its two-byte JSON body.
            while !got.ends_with(b"{}") {
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0, "the request ended early");
                got.extend_from_slice(&chunk[..read]);
            }
            let reply = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).unwrap();
            String::from_utf8(got).unwrap()
        });
        (url, handle)
    }

    #[tokio::test]
    async fn the_request_is_the_clis() {
        let (url, server) = serve("200 OK", r#"{"planUsage":{"totalPercentUsed":42}}"#);
        let client = http::client(false).unwrap();
        let body = fetch(&client, &url, "test-token").await.unwrap();
        assert_eq!(body["planUsage"]["totalPercentUsed"], 42);
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("post /aiserver.v1.dashboardservice/getcurrentperiodusage "));
        assert!(request.contains("authorization: bearer test-token\r\n"));
        assert!(request.contains("connect-protocol-version: 1\r\n"));
        assert!(request.contains("x-cursor-client-type: cli\r\n"));
        assert!(request.ends_with("\r\n\r\n{}"));
    }

    #[tokio::test]
    async fn a_turned_down_login_says_so_without_the_answer() {
        let (url, server) = serve("401 Unauthorized", "private answer");
        let client = http::client(false).unwrap();
        assert_eq!(
            fetch(&client, &url, "test-token").await,
            Err(Failure::Rejected)
        );
        server.join().unwrap();

        let (url, server) = serve("500 Internal Server Error", "private answer");
        let Err(Failure::Other(reason)) = fetch(&client, &url, "test-token").await else {
            panic!("expected a failure");
        };
        assert_eq!(reason, "Cursor answered 500");
        server.join().unwrap();

        let (url, server) = serve("200 OK", "private answer");
        let Err(Failure::Other(reason)) = fetch(&client, &url, "test-token").await else {
            panic!("expected a failure");
        };
        assert!(!reason.contains("private"), "{reason}");
        server.join().unwrap();
    }
}
