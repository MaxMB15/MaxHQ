// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the schedule shapes
// the tool takes, an interval of at least a minute, a time of day as HH:MM
// with weekdays where 0 is Sunday, a schedule sent as JSON text, the search
// a week ahead for the next fixed time, the 10 minutes before a fixed time
// counts as missed, and the webhook's filter and dedupe limits. MaxHQ adds a
// time zone to each fixed time and GitHub events as a trigger.

//! When a scheduled task runs. This reads a schedule from the tool's JSON,
//! checks a trigger and finds its next run. It does no I/O of its own:
//! jiff reads the time zone database.

use jiff::civil;
use jiff::tz::{AmbiguousOffset, TimeZone};
use maxhq_protocol::tasks::{
    DigestEncoding, Filter, FilterTest, Hook, HookRules, SignatureCheck, TimeOfDay, Trigger,
    Weekday, Zone,
};
use maxhq_protocol::time::Timestamp;
use serde::Deserialize;
use serde_json::Value;

use crate::github_events::REPO_PEOPLE;
use crate::hooks::dedupe;

/// The shortest interval, in milliseconds.
pub const MIN_EVERY_MS: u64 = 60_000;

/// How late a fixed time may run before it counts as missed, in
/// milliseconds.
pub const MISSED_AFTER_MS: u64 = 10 * 60_000;

/// The most filters a webhook or GitHub events trigger takes.
pub const MAX_FILTERS: usize = 16;

/// The oldest a request may be, in minutes, which is how long a held one
/// waits.
pub const MAX_AGE_MINUTES: u32 = 24 * 60;

/// The shortest and longest coalesce window, in seconds.
const COALESCE_SECS: (u32, u32) = (10, 3600);

/// The longest a window that restarts may wait, in seconds.
const MAX_WAIT_SECS: u32 = 24 * 60 * 60;

/// A schedule as the tool takes it.
#[derive(Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Shape {
    Interval {
        every_ms: u64,
    },
    FixedTime {
        time_of_day: String,
        #[serde(default)]
        weekdays: Vec<u8>,
        #[serde(default)]
        time_zone: Option<String>,
    },
    Webhook {
        #[serde(default)]
        signature: Option<SignatureShape>,
        #[serde(default)]
        max_delivery_age_minutes: Option<u32>,
        #[serde(default)]
        dedupe: Option<Dedupe>,
        #[serde(default)]
        filters: Vec<FilterShape>,
    },
    GithubEvents {
        repo: String,
        events: Vec<String>,
        #[serde(default)]
        max_delivery_age_minutes: Option<u32>,
        #[serde(default)]
        dedupe: Option<Dedupe>,
        #[serde(default)]
        filters: Vec<FilterShape>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignatureShape {
    header: String,
    encoding: DigestEncoding,
    #[serde(default)]
    prefix: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Dedupe {
    #[serde(default)]
    replay_key: Option<String>,
    #[serde(default)]
    coalesce_key: Option<String>,
    #[serde(default)]
    coalesce_window_seconds: Option<u32>,
    #[serde(default)]
    restart_window_on_each_request: bool,
    #[serde(default)]
    max_wait_seconds: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FilterShape {
    path: String,
    op: String,
    #[serde(default)]
    value: Value,
}

/// Reads a schedule from the tool's JSON, as an object or as JSON text,
/// and checks it. A fixed time with no `timeZone` takes `zone`.
pub fn parse(schedule: &Value, zone: &Zone) -> Result<Trigger, String> {
    let read;
    let schedule = match schedule {
        Value::String(text) => {
            read = serde_json::from_str::<Value>(text)
                .map_err(|_| "`schedule` must be an object.".to_owned())?;
            &read
        }
        value => value,
    };
    if schedule.pointer("/signature/secret").is_some()
        || schedule.pointer("/signature/secretRef").is_some()
    {
        return Err(
            "An agent can't set a webhook's secret. Ask the user to set it in MaxHQ.".into(),
        );
    }
    let shape: Shape = serde_json::from_value(schedule.clone())
        .map_err(|err| format!("`schedule` isn't one MaxHQ takes: {err}."))?;
    let trigger = match shape {
        Shape::Interval { every_ms } => {
            if every_ms < MIN_EVERY_MS {
                return Err(format!("`everyMs` must be at least {MIN_EVERY_MS}."));
            }
            let minutes = u32::try_from(every_ms.div_ceil(MIN_EVERY_MS))
                .map_err(|_| "`everyMs` is too long.".to_owned())?;
            Trigger::Every { minutes }
        }
        Shape::FixedTime {
            time_of_day,
            weekdays,
            time_zone,
        } => {
            let time = time_of_day_from(&time_of_day)
                .ok_or("`timeOfDay` must be HH:MM, such as 09:30.")?;
            let mut days = Vec::new();
            for day in weekdays {
                let day = weekday_from(day).ok_or("Each weekday must be 0 to 6, with 0 Sunday.")?;
                if !days.contains(&day) {
                    days.push(day);
                }
            }
            days.sort_by_key(|day| *day as u8);
            if days.len() == 7 {
                days.clear();
            }
            Trigger::At {
                time,
                days: maxhq_protocol::tasks::Days(days),
                zone: time_zone.map_or_else(|| zone.clone(), Zone),
            }
        }
        Shape::Webhook {
            signature,
            max_delivery_age_minutes,
            dedupe,
            filters,
        } => Trigger::Hook(Hook {
            on: false,
            signature: signature.map(|signature| SignatureCheck {
                header: signature.header.trim().to_lowercase(),
                encoding: signature.encoding,
                prefix: signature.prefix,
                has_secret: false,
            }),
            rules: rules_from(max_delivery_age_minutes, dedupe, filters)?,
            hold: false,
        }),
        Shape::GithubEvents {
            repo,
            events,
            max_delivery_age_minutes,
            dedupe,
            filters,
        } => Trigger::GithubEvents {
            repo: repo.trim().to_owned(),
            events: events.iter().map(|event| event.trim().to_owned()).collect(),
            rules: HookRules {
                // A task an agent makes runs only for the repository's
                // people. A person can open it up in the task's editor.
                authors: REPO_PEOPLE.map(str::to_owned).to_vec(),
                ..rules_from(max_delivery_age_minutes, dedupe, filters)?
            },
        },
    };
    validate(&trigger)?;
    Ok(trigger)
}

/// Checks a trigger, from the tool or the app.
pub fn validate(trigger: &Trigger) -> Result<(), String> {
    match trigger {
        Trigger::Every { minutes } => {
            if *minutes == 0 {
                return Err("A task runs at most once a minute.".into());
            }
        }
        Trigger::At { time, zone, .. } => {
            if time.hour > 23 || time.minute > 59 {
                return Err("The time must be from 00:00 to 23:59.".into());
            }
            if TimeZone::get(&zone.0).is_err() {
                return Err(format!("{:?} isn't a time zone MaxHQ knows.", zone.0));
            }
        }
        Trigger::Hook(hook) => {
            if let Some(signature) = &hook.signature
                && signature.header.trim().is_empty()
            {
                return Err("Name the header the signature comes in.".into());
            }
            check_rules(&hook.rules)?;
        }
        Trigger::GithubEvents {
            repo,
            events,
            rules,
        } => {
            if !is_repo(repo) {
                return Err("`repo` must be owner/name.".into());
            }
            if events.is_empty() || events.iter().any(|event| event.trim().is_empty()) {
                return Err("Name at least one event type, such as issues.".into());
            }
            check_rules(rules)?;
        }
    }
    Ok(())
}

/// When a trigger fires next after `after`, or none for a webhook,
/// GitHub events or a time zone that's gone. An interval counts elapsed
/// time, so it doesn't move with daylight saving. A fixed time in a gap
/// runs at the first minute after it, and one in a fold runs the first
/// time.
pub fn next_run(trigger: &Trigger, after: Timestamp) -> Option<Timestamp> {
    match trigger {
        Trigger::Every { minutes } => Some(Timestamp(
            after.0.saturating_add(u64::from(*minutes) * MIN_EVERY_MS),
        )),
        Trigger::At { time, days, zone } => {
            let tz = TimeZone::get(&zone.0).ok()?;
            let from = jiff::Timestamp::from_millisecond(i64::try_from(after.0).ok()?).ok()?;
            let today = from.to_zoned(tz.clone()).date();
            (0..=7).find_map(|offset| {
                let day = today.checked_add(jiff::Span::new().days(offset)).ok()?;
                if !days.0.is_empty() && !days.0.contains(&weekday_of(day)) {
                    return None;
                }
                let at = first_valid(&tz, day.at(time.hour as i8, time.minute as i8, 0, 0))?;
                let at = Timestamp(u64::try_from(at.as_millisecond()).ok()?);
                (at > after).then_some(at)
            })
        }
        Trigger::Hook(_) | Trigger::GithubEvents { .. } => None,
    }
}

/// Whether a fixed time due at `due` is too late to run at `now`.
pub fn is_missed(due: Timestamp, now: Timestamp) -> bool {
    now.0.saturating_sub(due.0) > MISSED_AFTER_MS
}

/// The instant a civil time names in `tz`: the first valid one after a
/// gap, and the earlier of a fold.
fn first_valid(tz: &TimeZone, at: civil::DateTime) -> Option<jiff::Timestamp> {
    match tz.to_ambiguous_timestamp(at).offset() {
        AmbiguousOffset::Unambiguous { offset } | AmbiguousOffset::Fold { before: offset, .. } => {
            offset.to_timestamp(at).ok()
        }
        // Read with the later offset, the time falls before the gap opens,
        // so the next transition is the gap's end.
        AmbiguousOffset::Gap { after, .. } => {
            let early = after.to_timestamp(at).ok()?;
            tz.following(early).next().map(|shift| shift.timestamp())
        }
    }
}

/// HH:MM, with a one-digit hour allowed.
fn time_of_day_from(text: &str) -> Option<TimeOfDay> {
    let (hour, minute) = text.trim().split_once(':')?;
    let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    if !(1..=2).contains(&hour.len()) || minute.len() != 2 || !digits(hour) || !digits(minute) {
        return None;
    }
    let (hour, minute) = (hour.parse().ok()?, minute.parse().ok()?);
    (hour < 24 && minute < 60).then_some(TimeOfDay { hour, minute })
}

/// The weekday number the tool takes, where 0 is Sunday.
fn weekday_from(day: u8) -> Option<Weekday> {
    Some(match day {
        0 => Weekday::Sunday,
        1 => Weekday::Monday,
        2 => Weekday::Tuesday,
        3 => Weekday::Wednesday,
        4 => Weekday::Thursday,
        5 => Weekday::Friday,
        6 => Weekday::Saturday,
        _ => return None,
    })
}

/// The day of the week a date falls on.
fn weekday_of(date: civil::Date) -> Weekday {
    match date.weekday() {
        civil::Weekday::Monday => Weekday::Monday,
        civil::Weekday::Tuesday => Weekday::Tuesday,
        civil::Weekday::Wednesday => Weekday::Wednesday,
        civil::Weekday::Thursday => Weekday::Thursday,
        civil::Weekday::Friday => Weekday::Friday,
        civil::Weekday::Saturday => Weekday::Saturday,
        civil::Weekday::Sunday => Weekday::Sunday,
    }
}

/// The rules a webhook or GitHub events schedule gives.
fn rules_from(
    max_age_minutes: Option<u32>,
    dedupe: Option<Dedupe>,
    filters: Vec<FilterShape>,
) -> Result<HookRules, String> {
    let dedupe = dedupe.unwrap_or(Dedupe {
        replay_key: None,
        coalesce_key: None,
        coalesce_window_seconds: None,
        restart_window_on_each_request: false,
        max_wait_seconds: None,
    });
    let filters = filters
        .into_iter()
        .map(filter_from)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(HookRules {
        filters,
        replay_key: dedupe.replay_key,
        coalesce_key: dedupe.coalesce_key,
        coalesce_secs: dedupe.coalesce_window_seconds,
        coalesce_restart: dedupe.restart_window_on_each_request,
        coalesce_max_secs: dedupe.max_wait_seconds,
        max_age_minutes,
        daily_runs: None,
        // A task an agent makes holds no free text. A person can allow
        // some in the task's editor.
        fields: Some(Vec::new()),
        authors: Vec::new(),
    })
}

/// One filter, from `{path, op, value}`.
fn filter_from(shape: FilterShape) -> Result<Filter, String> {
    let text = || match &shape.value {
        Value::String(value) => Ok(value.clone()),
        _ => Err(format!(
            "The filter on {} needs a text `value`.",
            shape.path
        )),
    };
    let test = match shape.op.as_str() {
        "equals" => FilterTest::Equals { value: text()? },
        "notEquals" => FilterTest::NotEquals { value: text()? },
        "startsWith" => FilterTest::StartsWith { value: text()? },
        "in" => FilterTest::In {
            values: serde_json::from_value(shape.value.clone())
                .map_err(|_| format!("The `in` filter on {} needs a list of text.", shape.path))?,
        },
        "exists" => FilterTest::Exists,
        op => {
            return Err(format!(
                "A filter's `op` can't be {op:?}: use equals, notEquals, startsWith, in or exists."
            ))
        }
    };
    Ok(Filter {
        path: shape.path.trim().to_owned(),
        test,
    })
}

/// Checks a webhook's or GitHub events' rules.
fn check_rules(rules: &HookRules) -> Result<(), String> {
    if rules.filters.len() > MAX_FILTERS {
        return Err(format!("Give at most {MAX_FILTERS} filters."));
    }
    for filter in &rules.filters {
        let root = filter.path.split('.').next().unwrap_or_default();
        if !matches!(root, "body" | "headers" | "query") {
            return Err(format!(
                "A filter's path starts with body, headers or query, not {:?}.",
                filter.path
            ));
        }
        if let FilterTest::In { values } = &filter.test
            && (values.is_empty()
                || values
                    .iter()
                    .any(|value| value.is_empty() || value.contains(',') || value.trim() != value))
        {
            return Err(format!(
                "The `in` filter on {} needs values without commas or spaces around them.",
                filter.path
            ));
        }
    }
    if let Some(secs) = rules.coalesce_secs
        && !(COALESCE_SECS.0..=COALESCE_SECS.1).contains(&secs)
    {
        return Err(format!(
            "The coalesce window is {} to {} seconds.",
            COALESCE_SECS.0, COALESCE_SECS.1
        ));
    }
    let window = rules.coalesce_secs.unwrap_or(dedupe::COALESCE_SECS);
    if let Some(secs) = rules.coalesce_max_secs
        && !(window..=MAX_WAIT_SECS).contains(&secs)
    {
        return Err(format!(
            "The longest wait is {window} to {MAX_WAIT_SECS} seconds."
        ));
    }
    if let Some(minutes) = rules.max_age_minutes
        && !(1..=MAX_AGE_MINUTES).contains(&minutes)
    {
        return Err(format!(
            "The oldest a request may be is 1 to {MAX_AGE_MINUTES} minutes."
        ));
    }
    Ok(())
}

/// Whether `repo` is owner/name, as GitHub spells them.
fn is_repo(repo: &str) -> bool {
    let part = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    repo.split_once('/')
        .is_some_and(|(owner, name)| part(owner) && part(name))
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::tasks::Days;
    use serde_json::json;

    use super::*;

    fn zone() -> Zone {
        Zone("Europe/Amsterdam".into())
    }

    fn at(text: &str) -> Timestamp {
        let ts: jiff::Timestamp = text.parse().unwrap();
        Timestamp(u64::try_from(ts.as_millisecond()).unwrap())
    }

    fn daily(hour: u8, minute: u8, days: Vec<Weekday>) -> Trigger {
        Trigger::At {
            time: TimeOfDay { hour, minute },
            days: Days(days),
            zone: zone(),
        }
    }

    #[test]
    fn an_interval_reads_whole_minutes_and_has_a_floor() {
        let every = parse(&json!({"type": "interval", "everyMs": 3_600_000}), &zone());
        assert_eq!(every, Ok(Trigger::Every { minutes: 60 }));
        let odd = parse(&json!({"type": "interval", "everyMs": 90_000}), &zone());
        assert_eq!(odd, Ok(Trigger::Every { minutes: 2 }));
        let short = parse(&json!({"type": "interval", "everyMs": 59_999}), &zone());
        assert!(short.unwrap_err().contains("at least 60000"));
    }

    #[test]
    fn a_fixed_time_reads_sunday_as_zero_and_takes_the_zone() {
        let schedule = json!({"type": "fixed_time", "timeOfDay": "9:05", "weekdays": [1, 0, 1]});
        assert_eq!(
            parse(&schedule, &zone()),
            Ok(daily(9, 5, vec![Weekday::Monday, Weekday::Sunday]))
        );
        let own = json!({"type": "fixed_time", "timeOfDay": "23:59", "timeZone": "UTC"});
        let Ok(Trigger::At { zone, days, .. }) = parse(&own, &Zone("Europe/Amsterdam".into()))
        else {
            panic!("it reads");
        };
        assert_eq!(zone, Zone("UTC".into()));
        assert!(days.0.is_empty());
        let every_day =
            json!({"type": "fixed_time", "timeOfDay": "08:00", "weekdays": [0, 1, 2, 3, 4, 5, 6]});
        assert_eq!(parse(&every_day, &self::zone()), Ok(daily(8, 0, vec![])));
    }

    #[test]
    fn bad_schedules_say_what_is_wrong() {
        let cases = [
            (json!({"type": "fixed_time", "timeOfDay": "24:00"}), "HH:MM"),
            (json!({"type": "fixed_time", "timeOfDay": "9:5"}), "HH:MM"),
            (
                json!({"type": "fixed_time", "timeOfDay": "09:00", "weekdays": [7]}),
                "0 to 6",
            ),
            (
                json!({"type": "fixed_time", "timeOfDay": "09:00", "timeZone": "Mars/Base"}),
                "time zone",
            ),
            (json!({"type": "cron", "expr": "* * * * *"}), "isn't one"),
            (
                json!({"type": "interval", "everyMs": 60000, "extra": 1}),
                "isn't one",
            ),
            (
                json!({"type": "webhook", "signature": {"header": "x-sig", "encoding": "hex", "secret": "s"}}),
                "secret",
            ),
            (
                json!({"type": "webhook", "filters": [{"path": "cookie.x", "op": "exists"}]}),
                "body, headers or query",
            ),
            (
                json!({"type": "webhook", "filters": [{"path": "body.a", "op": "matches", "value": "x"}]}),
                "can't be",
            ),
            (
                json!({"type": "webhook", "dedupe": {"coalesceWindowSeconds": 5}}),
                "coalesce window",
            ),
            (
                json!({"type": "webhook", "dedupe": {"coalesceWindowSeconds": 900, "maxWaitSeconds": 600}}),
                "900 to 86400",
            ),
            (
                json!({"type": "webhook", "dedupe": {"maxWaitSeconds": 100000}}),
                "120 to 86400",
            ),
            (
                json!({"type": "github_events", "repo": "nope", "events": ["issues"]}),
                "owner/name",
            ),
            (
                json!({"type": "github_events", "repo": "owner/repo", "events": []}),
                "event type",
            ),
        ];
        for (schedule, says) in cases {
            let err = parse(&schedule, &zone()).expect_err("it's refused");
            assert!(err.contains(says), "{schedule}: {err}");
        }
    }

    #[test]
    fn a_webhook_reads_its_rules_even_as_json_text() {
        let schedule = json!({
            "type": "webhook",
            "signature": {"header": "X-Hub-Signature-256", "encoding": "hex", "prefix": "sha256="},
            "maxDeliveryAgeMinutes": 30,
            "dedupe": {"replayKey": "{{body.id}}"},
            "filters": [
                {"path": "headers.x-github-event", "op": "equals", "value": "issues"},
                {"path": "body.action", "op": "in", "value": ["opened", "edited"]},
            ],
        });
        let text = Value::String(schedule.to_string());
        let Ok(Trigger::Hook(hook)) = parse(&text, &zone()) else {
            panic!("it reads");
        };
        assert!(!hook.on && !hook.hold);
        let signature = hook.signature.expect("it's signed");
        assert_eq!(signature.header, "x-hub-signature-256");
        assert!(!signature.has_secret);
        assert_eq!(hook.rules.max_age_minutes, Some(30));
        assert_eq!(hook.rules.replay_key.as_deref(), Some("{{body.id}}"));
        assert_eq!(
            hook.rules.filters[1].test,
            FilterTest::In {
                values: vec!["opened".into(), "edited".into()]
            }
        );
        let events = json!({"type": "github_events", "repo": "owner/repo", "events": ["issues"]});
        assert!(matches!(
            parse(&events, &zone()),
            Ok(Trigger::GithubEvents { .. })
        ));
    }

    #[test]
    fn a_github_task_an_agent_makes_keeps_to_the_repos_people() {
        let events = json!({"type": "github_events", "repo": "owner/repo", "events": ["issues"]});
        let Ok(Trigger::GithubEvents { rules, .. }) = parse(&events, &zone()) else {
            panic!("a GitHub events schedule");
        };
        assert_eq!(rules.authors, ["OWNER", "MEMBER", "COLLABORATOR"]);

        let hook = json!({"type": "webhook"});
        let Ok(Trigger::Hook(hook)) = parse(&hook, &zone()) else {
            panic!("a webhook schedule");
        };
        assert!(hook.rules.authors.is_empty());
    }

    #[test]
    fn a_debounce_restarts_with_each_event() {
        let schedule = json!({
            "type": "github_events",
            "repo": "owner/repo",
            "events": ["pull_request"],
            "dedupe": {
                "coalesceKey": "all",
                "coalesceWindowSeconds": 900,
                "restartWindowOnEachRequest": true,
            },
        });
        let Ok(Trigger::GithubEvents { rules, .. }) = parse(&schedule, &zone()) else {
            panic!("it reads");
        };
        assert_eq!(rules.coalesce_key.as_deref(), Some("all"));
        assert_eq!(rules.coalesce_secs, Some(900));
        assert!(rules.coalesce_restart);
        assert_eq!(rules.coalesce_max_secs, None);

        let mut capped = schedule.clone();
        capped["dedupe"]["maxWaitSeconds"] = json!(3600);
        let Ok(Trigger::GithubEvents { rules, .. }) = parse(&capped, &zone()) else {
            panic!("it reads");
        };
        assert_eq!(rules.coalesce_max_secs, Some(3600));
    }

    #[test]
    fn an_interval_counts_elapsed_time() {
        let after = at("2026-03-29T00:30:00Z");
        assert_eq!(
            next_run(&Trigger::Every { minutes: 60 }, after),
            Some(at("2026-03-29T01:30:00Z"))
        );
        assert_eq!(next_run(&Trigger::Hook(Hook::default()), after), None);
    }

    #[test]
    fn a_fixed_time_runs_today_or_the_next_day() {
        let trigger = daily(9, 30, vec![]);
        // 08:00 in Amsterdam, in winter.
        assert_eq!(
            next_run(&trigger, at("2026-01-14T07:00:00Z")),
            Some(at("2026-01-14T08:30:00Z"))
        );
        assert_eq!(
            next_run(&trigger, at("2026-01-14T08:30:00Z")),
            Some(at("2026-01-15T08:30:00Z"))
        );
    }

    #[test]
    fn a_fixed_time_waits_for_its_weekday() {
        // 2026-01-14 is a Wednesday.
        let sunday = daily(9, 0, vec![Weekday::Sunday]);
        assert_eq!(
            next_run(&sunday, at("2026-01-14T12:00:00Z")),
            Some(at("2026-01-18T08:00:00Z"))
        );
        let weekdays = parse(
            &json!({"type": "fixed_time", "timeOfDay": "09:00", "weekdays": [1, 2, 3, 4, 5]}),
            &zone(),
        )
        .unwrap();
        // From Friday after nine, the next is Monday.
        assert_eq!(
            next_run(&weekdays, at("2026-01-16T09:00:00Z")),
            Some(at("2026-01-19T08:00:00Z"))
        );
    }

    #[test]
    fn a_fixed_time_keeps_its_local_time_across_both_changes() {
        let trigger = daily(9, 0, vec![]);
        // Summer time starts on 2026-03-29 and ends on 2026-10-25.
        assert_eq!(
            next_run(&trigger, at("2026-03-28T09:00:00Z")),
            Some(at("2026-03-29T07:00:00Z"))
        );
        assert_eq!(
            next_run(&trigger, at("2026-10-24T08:00:00Z")),
            Some(at("2026-10-25T08:00:00Z"))
        );
    }

    #[test]
    fn a_time_in_the_gap_runs_at_its_end_and_one_in_the_fold_runs_once_first() {
        let gap = daily(2, 30, vec![]);
        // 02:30 doesn't exist on 2026-03-29: clocks go from 02:00 to 03:00.
        assert_eq!(
            next_run(&gap, at("2026-03-28T12:00:00Z")),
            Some(at("2026-03-29T01:00:00Z"))
        );
        let fold = daily(2, 30, vec![]);
        // 02:30 happens twice on 2026-10-25, first at +02:00.
        assert_eq!(
            next_run(&fold, at("2026-10-24T12:00:00Z")),
            Some(at("2026-10-25T00:30:00Z"))
        );
        assert_eq!(
            next_run(&fold, at("2026-10-25T00:30:00Z")),
            Some(at("2026-10-26T01:30:00Z"))
        );
    }

    #[test]
    fn a_fixed_time_is_missed_after_ten_minutes() {
        let due = Timestamp(1_000_000);
        assert!(!is_missed(due, Timestamp(due.0 + MISSED_AFTER_MS)));
        assert!(is_missed(due, Timestamp(due.0 + MISSED_AFTER_MS + 1)));
        assert!(!is_missed(due, Timestamp(due.0 - 1)));
    }
}
