// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What an OpenRouter key has spent and has left. OpenRouter answers
//! `GET /api/v1/key` with the figures of the key that asks, so `maxd` reads
//! them with the server's own token: per key. The account's balance, at
//! `GET /api/v1/credits`, needs an admin key (OpenRouter calls it a
//! management key), which `maxd` holds only when you add one and uses for
//! nothing else.
//!
//! A [`Meter`] keeps one server's figures and decides when to read them
//! again. It reads once a `maxd` has the server, on Test and right after the
//! server is added, after a turn on it ends but at most once a minute, and
//! every 15 minutes while an agent on it has run in the last hour. Nothing
//! else reads them. When OpenRouter answers 429 it waits twice as long each
//! time, up to an hour. A read that fails keeps the last good figures and
//! says why.
//!
//! The key and OpenRouter's answers are never logged.

use std::time::{Duration, Instant};

use maxhq_protocol::local_models::ServerCredits;
use maxhq_protocol::secret::Secret;
use maxhq_protocol::time::Timestamp;
use serde::Deserialize;

/// Where a key's own figures are, under the server's base URL.
const KEY_PATH: &str = "/v1/key";

/// Where the account's balance is, for an admin key.
const BALANCE_PATH: &str = "/v1/credits";

/// How long a read may take.
const LOOK: Duration = Duration::from_secs(10);

/// The least time between two reads a turn's end asks for.
pub const AFTER_TURN: Duration = Duration::from_secs(60);

/// How often the figures are read while agents on the server work.
pub const EVERY: Duration = Duration::from_secs(15 * 60);

/// How long after an agent's last turn the server counts as in use.
pub const IN_USE: Duration = Duration::from_secs(60 * 60);

/// The first wait after OpenRouter answers 429. Each one after doubles it.
const FIRST_WAIT: Duration = Duration::from_secs(60);

/// The longest wait after a 429.
const LONGEST_WAIT: Duration = Duration::from_secs(60 * 60);

/// What a failed turn says when the OpenRouter key's own limit is spent.
pub const KEY_LIMIT: &str =
    "This OpenRouter key reached its spending limit. Raise the key's limit \
                             on OpenRouter, or wait for the limit to reset, then send your \
                             message again.";

/// What a failed turn says when the OpenRouter account has no credits left.
pub const NO_CREDITS: &str = "Your OpenRouter account is out of credits. Add credits on \
                              OpenRouter, then send your message again.";

/// Why a read failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// OpenRouter answered 429, with how long it asked to wait, if it said.
    Busy(Option<Duration>),
    /// Anything else, for a person to read.
    Other(String),
}

impl Failure {
    /// Why, for a person to read.
    pub fn words(&self) -> String {
        match self {
            Self::Busy(_) => {
                "OpenRouter asked MaxHQ to wait before reading the credits again.".to_owned()
            }
            Self::Other(why) => why.clone(),
        }
    }
}

/// The part of OpenRouter's answer `maxd` reads, in US dollars.
#[derive(Deserialize)]
struct Answer<T> {
    data: T,
}

#[derive(Deserialize)]
struct KeyFigures {
    usage: f64,
    #[serde(default)]
    usage_daily: Option<f64>,
    #[serde(default)]
    usage_weekly: Option<f64>,
    #[serde(default)]
    usage_monthly: Option<f64>,
    #[serde(default)]
    limit: Option<f64>,
    #[serde(default)]
    limit_remaining: Option<f64>,
    #[serde(default)]
    limit_reset: Option<String>,
}

/// US dollars as whole cents, rounded, never below zero.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to the range of u32 first"
)]
fn cents(dollars: f64) -> u32 {
    if dollars.is_nan() {
        return 0;
    }
    (dollars * 100.0).round().clamp(0.0, f64::from(u32::MAX)) as u32
}

/// The account's figures, from [`BALANCE_PATH`], in US dollars.
#[derive(Deserialize)]
struct AccountFigures {
    total_credits: f64,
    total_usage: f64,
}

/// What the account has left and bought, in whole cents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Balance {
    pub balance_cents: u32,
    pub bought_cents: u32,
}

/// One of the two things `maxd` asks OpenRouter.
struct Ask {
    /// Where, under the server's base URL.
    path: &'static str,
    /// What it's for, in a failure's words.
    what: &'static str,
    /// What a failure says when OpenRouter turns the key down.
    refused: &'static str,
}

/// The key's own figures, which its own token reads.
const KEY: Ask = Ask {
    path: KEY_PATH,
    what: "the key's credits",
    refused: "OpenRouter didn't accept this key for its credits.",
};

/// The account's balance, which only an admin key reads.
const BALANCE: Ask = Ask {
    path: BALANCE_PATH,
    what: "the account's balance",
    refused: "OpenRouter didn't accept this admin key. Use a management key from your \
              OpenRouter settings.",
};

/// Asks OpenRouter at `base` for what `ask` names, with `key`.
async fn ask<T: serde::de::DeserializeOwned>(
    base: &str,
    ask: &Ask,
    key: &Secret,
) -> Result<T, Failure> {
    let url = format!("{}{}", base.trim_end_matches('/'), ask.path);
    let answer = super::http()
        .get(url)
        .header("accept", "application/json")
        .bearer_auth(key.expose())
        .timeout(LOOK)
        .send()
        .await
        .map_err(|err| {
            Failure::Other(format!(
                "Couldn't reach OpenRouter: {}.",
                crate::tools::install::fetch::plain(&err)
            ))
        })?;
    let status = answer.status().as_u16();
    if status == 429 {
        let wait = answer
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        return Err(Failure::Busy(wait));
    }
    if matches!(status, 401 | 403) {
        return Err(Failure::Other(ask.refused.to_owned()));
    }
    if !answer.status().is_success() {
        return Err(Failure::Other(format!(
            "OpenRouter answered {status} when asked for {}.",
            ask.what
        )));
    }
    let unread = || {
        Failure::Other(format!(
            "Couldn't find {} in OpenRouter's answer.",
            ask.what
        ))
    };
    let text = answer.text().await.map_err(|_| unread())?;
    serde_json::from_str::<Answer<T>>(&text)
        .map(|answer| answer.data)
        .map_err(|_| unread())
}

/// Reads the account's balance at the server `base` with the admin key
/// `admin`: what was bought, and that less what was used.
pub async fn balance(base: &str, admin: &Secret) -> Result<Balance, Failure> {
    let figures: AccountFigures = ask(base, &BALANCE, admin).await?;
    Ok(Balance {
        balance_cents: cents(figures.total_credits - figures.total_usage),
        bought_cents: cents(figures.total_credits),
    })
}

/// Reads the figures of the key `token` at the server `base`, and the
/// account's balance too with the admin key `admin`, when there is one.
pub async fn read(
    base: &str,
    token: &Secret,
    admin: Option<&Secret>,
) -> Result<ServerCredits, Failure> {
    let figures: KeyFigures = ask(base, &KEY, token).await?;
    let account = match admin {
        Some(admin) => Some(balance(base, admin).await?),
        None => None,
    };
    Ok(ServerCredits {
        at: Timestamp::now(),
        used_cents: cents(figures.usage),
        today_cents: figures.usage_daily.map(cents),
        week_cents: figures.usage_weekly.map(cents),
        month_cents: figures.usage_monthly.map(cents),
        limit_cents: figures.limit.map(cents),
        remaining_cents: figures.limit_remaining.map(cents),
        resets: figures
            .limit_reset
            .map(|reset| reset.trim().to_owned())
            .filter(|reset| !reset.is_empty()),
        balance_cents: account.map(|account| account.balance_cents),
        bought_cents: account.map(|account| account.bought_cents),
        error: None,
    })
}

/// One server's figures and when to read them again.
#[derive(Debug)]
pub struct Meter {
    /// The figures to show: the last good ones, with why the last read
    /// failed when it did. None before a read worked.
    pub shown: Option<ServerCredits>,
    /// A read is wanted as soon as one may go: the first, or a Test.
    wanted: bool,
    /// A turn ended since the last read.
    turn_ended: bool,
    /// When an agent on the server last ended a turn.
    ran_at: Option<Instant>,
    /// When the last read went.
    read_at: Option<Instant>,
    /// Whether a read is on its way.
    reading: bool,
    /// No read goes before this, after a 429.
    hold_until: Option<Instant>,
    /// The wait after the last 429, which the next one doubles.
    wait: Option<Duration>,
}

impl Default for Meter {
    fn default() -> Self {
        Self {
            shown: None,
            wanted: true,
            turn_ended: false,
            ran_at: None,
            read_at: None,
            reading: false,
            hold_until: None,
            wait: None,
        }
    }
}

impl Meter {
    /// Asks for a read as soon as one may go, as on Test.
    pub fn want(&mut self) {
        self.wanted = true;
    }

    /// Notes that a turn on the server ended at `now`.
    pub fn turn_ended(&mut self, now: Instant) {
        self.turn_ended = true;
        self.ran_at = Some(now);
    }

    /// Whether a read is due at `now`. `working` says whether an agent on
    /// the server works now, and is asked only when it matters.
    pub fn due(&self, now: Instant, working: impl FnOnce() -> bool) -> bool {
        if self.reading || self.hold_until.is_some_and(|until| now < until) {
            return false;
        }
        if self.wanted {
            return true;
        }
        let since = self.read_at.map(|at| now.saturating_duration_since(at));
        if self.turn_ended && since.is_none_or(|since| since >= AFTER_TURN) {
            return true;
        }
        since.is_none_or(|since| since >= EVERY)
            && (self
                .ran_at
                .is_some_and(|at| now.saturating_duration_since(at) < IN_USE)
                || working())
    }

    /// Notes that a read starts at `now`.
    pub fn start(&mut self, now: Instant) {
        self.reading = true;
        self.wanted = false;
        self.turn_ended = false;
        self.read_at = Some(now);
    }

    /// Takes a read's result at `now`, and says whether the figures to
    /// show changed.
    pub fn finish(&mut self, result: Result<ServerCredits, Failure>, now: Instant) -> bool {
        self.reading = false;
        let before = self.shown.clone();
        match result {
            Ok(figures) => {
                self.wait = None;
                self.hold_until = None;
                self.shown = Some(figures);
            }
            Err(failure) => {
                if let Failure::Busy(asked) = failure {
                    let wait = self
                        .wait
                        .map_or(FIRST_WAIT, |wait| wait.saturating_mul(2))
                        .min(LONGEST_WAIT);
                    self.wait = Some(wait);
                    let asked = asked.unwrap_or_default().min(LONGEST_WAIT);
                    self.hold_until = Some(now + wait.max(asked));
                }
                if let Some(shown) = &mut self.shown {
                    shown.error = Some(failure.words());
                }
            }
        }
        self.shown != before
    }

    /// Shows the account's `balance` with the key's figures, or none when
    /// the admin key went, and says whether that changed what's shown.
    pub fn set_balance(&mut self, balance: Option<Balance>) -> bool {
        let Some(shown) = &mut self.shown else {
            return false;
        };
        let before = (shown.balance_cents, shown.bought_cents);
        shown.balance_cents = balance.map(|balance| balance.balance_cents);
        shown.bought_cents = balance.map(|balance| balance.bought_cents);
        before != (shown.balance_cents, shown.bought_cents)
    }
}

/// What a failed turn says in plain words when OpenRouter turned it down
/// over money, from the `limit_source` its error carries. `text` is what
/// Claude Code wrote for the failure, which holds OpenRouter's error. None
/// for any other failure, which keeps its own text.
pub fn turned_down(text: &str) -> Option<&'static str> {
    if !text.contains("limit_source") {
        return None;
    }
    if text.contains("openrouter_key_limit") {
        Some(KEY_LIMIT)
    } else if text.contains("openrouter_credits") || text.contains("openrouter_in_flight_budget") {
        Some(NO_CREDITS)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::tests::Fake;
    use super::*;

    fn token() -> Secret {
        Secret::new("sk-or-v1-example".to_owned())
    }

    /// OpenRouter's answer for a key with a monthly $10 limit.
    fn limited() -> serde_json::Value {
        json!({"data": {
            "label": "sk-or-v1-exa...ple",
            "limit": 10,
            "limit_reset": "monthly",
            "limit_remaining": 7.4249,
            "include_byok_in_limit": false,
            "usage": 2.5751,
            "usage_daily": 0.314,
            "usage_weekly": 1.2,
            "usage_monthly": 2.5751,
            "byok_usage": 0,
            "is_free_tier": false,
        }})
    }

    #[tokio::test]
    async fn a_key_s_figures_read_in_whole_cents_with_its_token() {
        let fake = Fake::start().await;
        fake.json("/api/v1/key", 200, &limited());
        let base = format!("{}/api", fake.base);
        let read = read(&base, &token(), None).await.unwrap();
        assert_eq!(read.used_cents, 258);
        assert_eq!(read.today_cents, Some(31));
        assert_eq!(read.week_cents, Some(120));
        assert_eq!(read.month_cents, Some(258));
        assert_eq!(read.limit_cents, Some(1_000));
        assert_eq!(read.remaining_cents, Some(742));
        assert_eq!(read.resets.as_deref(), Some("monthly"));
        assert_eq!(read.balance_cents, None, "no admin key, no balance");
        assert_eq!(read.error, None);
        let seen = fake.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].path, "/api/v1/key");
        assert_eq!(
            seen[0].header("authorization"),
            Some("Bearer sk-or-v1-example")
        );
    }

    fn admin() -> Secret {
        Secret::new("sk-or-v1-admin".to_owned())
    }

    #[tokio::test]
    async fn an_admin_key_reads_the_account_s_balance_too() {
        let fake = Fake::start().await;
        fake.json("/api/v1/key", 200, &limited());
        fake.json(
            "/api/v1/credits",
            200,
            &json!({"data": {"total_credits": 10, "total_usage": 2.5751}}),
        );
        let base = format!("{}/api", fake.base);
        let both = read(&base, &token(), Some(&admin())).await.unwrap();
        assert_eq!(both.used_cents, 258);
        assert_eq!(both.balance_cents, Some(742));
        assert_eq!(both.bought_cents, Some(1_000));
        let seen = fake.seen();
        let asked = seen
            .iter()
            .find(|seen| seen.path == "/api/v1/credits")
            .unwrap();
        assert_eq!(asked.header("authorization"), Some("Bearer sk-or-v1-admin"));
        let key = seen.iter().find(|seen| seen.path == "/api/v1/key").unwrap();
        assert_eq!(key.header("authorization"), Some("Bearer sk-or-v1-example"));

        // Used past what was bought reads as nothing left.
        fake.json(
            "/api/v1/credits",
            200,
            &json!({"data": {"total_credits": 5, "total_usage": 5.4}}),
        );
        let over = balance(&base, &admin()).await.unwrap();
        assert_eq!(over.balance_cents, 0);
        assert_eq!(over.bought_cents, 500);

        // A normal key in its place is turned down, and the read fails.
        fake.json(
            "/api/v1/credits",
            403,
            &json!({"error": {"code": 403, "message": "Only management keys can perform this operation."}}),
        );
        let Err(Failure::Other(why)) = read(&base, &token(), Some(&admin())).await else {
            panic!("not refused");
        };
        assert!(why.contains("admin key"), "{why}");
        assert!(!why.contains("sk-or"));
        fake.json("/api/v1/credits", 429, &json!({}));
        assert_eq!(balance(&base, &admin()).await, Err(Failure::Busy(None)));
    }

    #[tokio::test]
    async fn a_key_without_a_limit_reads_what_it_spent() {
        let fake = Fake::start().await;
        fake.json(
            "/v1/key",
            200,
            &json!({"data": {"usage": 2.58, "usage_daily": 0.31, "limit": null, "limit_remaining": null, "limit_reset": null}}),
        );
        let read = read(&fake.base, &token(), None).await.unwrap();
        assert_eq!(read.used_cents, 258);
        assert_eq!(read.today_cents, Some(31));
        assert_eq!(read.week_cents, None);
        assert_eq!(read.limit_cents, None);
        assert_eq!(read.remaining_cents, None);
        assert_eq!(read.resets, None);
    }

    #[tokio::test]
    async fn failures_say_why_without_the_key() {
        let fake = Fake::start().await;
        fake.json(
            "/v1/key",
            401,
            &json!({"error": {"code": 401, "message": "sk-or-v1-example"}}),
        );
        let Err(Failure::Other(why)) = read(&fake.base, &token(), None).await else {
            panic!("not refused");
        };
        assert!(why.contains("didn't accept"), "{why}");
        assert!(!why.contains("sk-or"));

        fake.json("/v1/key", 429, &json!({"error": {"code": 429}}));
        assert_eq!(
            read(&fake.base, &token(), None).await,
            Err(Failure::Busy(None))
        );

        fake.json("/v1/key", 500, &json!({}));
        let Err(Failure::Other(why)) = read(&fake.base, &token(), None).await else {
            panic!("not failed");
        };
        assert!(why.contains("500"), "{why}");

        fake.json("/v1/key", 200, &json!({"data": {"label": "no figures"}}));
        let Err(Failure::Other(why)) = read(&fake.base, &token(), None).await else {
            panic!("read without figures");
        };
        assert!(why.contains("Couldn't find the key's credits"), "{why}");

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let gone = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let Err(Failure::Other(why)) = read(&gone, &token(), None).await else {
            panic!("reached nothing");
        };
        assert!(why.starts_with("Couldn't reach OpenRouter"), "{why}");
    }

    fn figures(used: u32) -> ServerCredits {
        ServerCredits {
            at: Timestamp(1_000),
            used_cents: used,
            today_cents: None,
            week_cents: None,
            month_cents: None,
            limit_cents: None,
            remaining_cents: None,
            resets: None,
            balance_cents: None,
            bought_cents: None,
            error: None,
        }
    }

    #[test]
    fn the_first_read_goes_at_once_and_then_only_when_something_asks() {
        let start = Instant::now();
        let mut meter = Meter::default();
        assert!(meter.due(start, || false));
        meter.start(start);
        assert!(!meter.due(start, || true), "one read at a time");
        assert!(meter.finish(Ok(figures(100)), start));
        // Nothing ran, so nothing reads, however long it waits.
        let later = start + EVERY * 4;
        assert!(!meter.due(later, || false));
        // Test reads at once.
        meter.want();
        assert!(meter.due(start + Duration::from_secs(1), || false));
    }

    #[test]
    fn a_turn_s_end_reads_at_most_once_a_minute() {
        let start = Instant::now();
        let mut meter = Meter::default();
        meter.start(start);
        meter.finish(Ok(figures(100)), start);
        let soon = start + Duration::from_secs(20);
        meter.turn_ended(soon);
        assert!(!meter.due(soon, || false));
        assert!(meter.due(start + AFTER_TURN, || false));
        meter.start(start + AFTER_TURN);
        meter.finish(Ok(figures(100)), start + AFTER_TURN);
        assert!(
            !meter.due(start + AFTER_TURN * 2, || false),
            "no turn ended"
        );
    }

    #[test]
    fn a_server_in_use_reads_every_fifteen_minutes() {
        let start = Instant::now();
        let mut meter = Meter::default();
        meter.start(start);
        meter.finish(Ok(figures(100)), start);
        meter.turn_ended(start);
        meter.start(start + AFTER_TURN);
        meter.finish(Ok(figures(120)), start + AFTER_TURN);
        let next = start + AFTER_TURN + EVERY;
        assert!(!meter.due(next - Duration::from_secs(1), || true));
        assert!(meter.due(next, || false), "it ran within the hour");
        // Past the hour only an agent at work keeps it reading.
        let idle = start + IN_USE + EVERY;
        assert!(!meter.due(idle, || false));
        assert!(meter.due(idle, || true));
    }

    #[test]
    fn a_429_waits_twice_as_long_each_time_up_to_an_hour_and_keeps_the_figures() {
        let start = Instant::now();
        let mut meter = Meter::default();
        meter.start(start);
        meter.finish(Ok(figures(100)), start);
        let mut now = start;
        let mut waits = Vec::new();
        for _ in 0..8 {
            meter.want();
            meter.start(now);
            meter.finish(Err(Failure::Busy(None)), now);
            let until = meter.hold_until.unwrap();
            waits.push((until - now).as_secs());
            meter.want();
            assert!(!meter.due(until - Duration::from_secs(1), || true));
            assert!(meter.due(until, || false), "the Test still waits for it");
            now = until;
        }
        assert_eq!(waits, [60, 120, 240, 480, 960, 1920, 3600, 3600]);
        let shown = meter.shown.clone().unwrap();
        assert_eq!(shown.used_cents, 100);
        assert!(shown.error.unwrap().contains("wait"));
        // A wait OpenRouter asks for counts when it's longer.
        meter.start(now);
        meter.finish(Ok(figures(110)), now);
        meter.start(now);
        meter.finish(Err(Failure::Busy(Some(Duration::from_secs(300)))), now);
        assert_eq!(meter.hold_until, Some(now + Duration::from_secs(300)));
        // A good read clears the wait and the error.
        meter.start(now + Duration::from_secs(300));
        assert!(meter.finish(Ok(figures(110)), now));
        assert_eq!(meter.shown.as_ref().unwrap().error, None);
        meter.start(now);
        meter.finish(Err(Failure::Busy(None)), now);
        assert_eq!(meter.hold_until, Some(now + FIRST_WAIT));
    }

    #[test]
    fn a_failed_read_keeps_the_last_good_figures_and_says_why() {
        let start = Instant::now();
        let mut meter = Meter::default();
        meter.start(start);
        assert!(!meter.finish(Err(Failure::Other("down".into())), start));
        assert_eq!(meter.shown, None, "nothing to show before a good read");
        meter.want();
        meter.start(start);
        meter.finish(Ok(figures(258)), start);
        meter.want();
        meter.start(start);
        assert!(meter.finish(Err(Failure::Other("down".into())), start));
        let shown = meter.shown.clone().unwrap();
        assert_eq!(shown.used_cents, 258);
        assert_eq!(shown.error.as_deref(), Some("down"));
        assert_eq!(meter.hold_until, None, "only a 429 holds reads back");
    }

    #[test]
    fn a_turn_openrouter_turned_down_over_money_says_so_plainly() {
        let key = r#"API Error: 403 {"error":{"message":"Key limit exceeded","code":403,"metadata":{"limit_source":"openrouter_key_limit","scope":"key"}}}"#;
        assert_eq!(turned_down(key), Some(KEY_LIMIT));
        let credits = r#"API Error: 402 {"error":{"code":402,"metadata":{"limit_source":"openrouter_credits"}}}"#;
        assert_eq!(turned_down(credits), Some(NO_CREDITS));
        let held = r#"API Error: 402 {"error":{"code":402,"metadata":{"limit_source":"openrouter_in_flight_budget"}}}"#;
        assert_eq!(turned_down(held), Some(NO_CREDITS));
        assert_eq!(turned_down("API Error: 402 Insufficient credits"), None);
        assert_eq!(
            turned_down(r#"{"metadata":{"limit_source":"openrouter_workspace_budget"}}"#),
            None
        );
        assert_eq!(turned_down("API Error: 500"), None);
    }
}
