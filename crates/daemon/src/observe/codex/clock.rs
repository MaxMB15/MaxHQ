// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The times a rollout holds. Lines carry UTC timestamps in RFC 3339, and
//! a usage limit's message says when to try again in the device's local
//! time, which `maxd` converts back through SQLite's date functions so it
//! needs no time zone crate.

use maxhq_protocol::time::Timestamp;
use rusqlite::Connection;

/// A day on the calendar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Date {
    /// The year, such as 2026.
    pub year: i64,
    /// From 1 for January to 12.
    pub month: u32,
    /// From 1.
    pub day: u32,
}

/// Converts between UTC and a local time zone.
pub trait LocalClock {
    /// The local date at an instant.
    fn date_at(&self, at: Timestamp) -> Option<Date>;
    /// The instant a local date and time of day falls on.
    fn to_utc(&self, date: Date, hour: u32, minute: u32) -> Option<Timestamp>;
}

/// The device's own time zone, through the C library that SQLite calls.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl LocalClock for SystemClock {
    fn date_at(&self, at: Timestamp) -> Option<Date> {
        let secs = i64::try_from(at.0 / 1000).ok()?;
        let text: String = Connection::open_in_memory()
            .ok()?
            .query_row("SELECT date(?1, 'unixepoch', 'localtime')", [secs], |row| {
                row.get(0)
            })
            .ok()?;
        parse_date(&text)
    }

    fn to_utc(&self, date: Date, hour: u32, minute: u32) -> Option<Timestamp> {
        let local = format!(
            "{:04}-{:02}-{:02} {hour:02}:{minute:02}:00",
            date.year, date.month, date.day
        );
        let secs: i64 = Connection::open_in_memory()
            .ok()?
            .query_row(
                "SELECT CAST(strftime('%s', ?1, 'utc') AS INTEGER)",
                [local],
                |row| row.get(0),
            )
            .ok()?;
        Some(Timestamp(u64::try_from(secs).ok()?.checked_mul(1000)?))
    }
}

/// Parses an RFC 3339 time such as `2026-10-19T20:11:26.828Z`. Digits past
/// milliseconds are dropped, and a missing offset reads as UTC.
pub fn parse_utc(text: &str) -> Option<Timestamp> {
    let date = parse_date(text.get(..10)?)?;
    if !matches!(text.as_bytes().get(10), Some(b'T' | b't' | b' ')) {
        return None;
    }
    let hour = digits(text, 11..13)?;
    let minute = digits(text, 14..16)?;
    let second = digits(text, 17..19)?;
    if text.get(13..14)? != ":" || text.get(16..17)? != ":" {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut rest = text.get(19..)?;
    let mut millis = 0;
    if let Some(fraction) = rest.strip_prefix('.') {
        let len = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if len == 0 {
            return None;
        }
        let kept = &fraction[..len.min(3)];
        let scale = 10_u64.pow(3 - u32::try_from(kept.len()).ok()?);
        millis = kept.parse::<u64>().ok()? * scale;
        rest = &fraction[len..];
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let hours = i64::from(digits(rest, 1..3)?);
            let minutes = match rest.len() {
                6 if rest.get(3..4) == Some(":") => i64::from(digits(rest, 4..6)?),
                5 => i64::from(digits(rest, 3..5)?),
                3 => 0,
                _ => return None,
            };
            sign * (hours * 3600 + minutes * 60)
        }
    };
    let secs = days_from_civil(date) * 86_400
        + i64::from(hour) * 3600
        + i64::from(minute) * 60
        + i64::from(second)
        - offset;
    let secs = u64::try_from(secs).ok()?;
    Some(Timestamp(secs.checked_mul(1000)?.checked_add(millis)?))
}

/// Reads when to try again from a usage limit's message, which ends with
/// "try again at 3:05 PM." on the day it was written and "try again at Oct
/// 27th, 2026 3:05 PM." for a later day. `written` is when the message was
/// written, which gives the day for the short form.
pub fn limit_resets_at(
    message: &str,
    written: Timestamp,
    clock: &impl LocalClock,
) -> Option<Timestamp> {
    /// The words just before the time.
    const MARKER: &str = "try again at ";
    let start = message.to_ascii_lowercase().rfind(MARKER)? + MARKER.len();
    let when = message.get(start..)?.trim_end().trim_end_matches('.');
    let words: Vec<&str> = when.split_whitespace().collect();
    let (date, clock_time, meridiem) = match words.as_slice() {
        [time, meridiem] => (clock.date_at(written)?, *time, *meridiem),
        [month, day, year, time, meridiem] => {
            let date = Date {
                year: year.parse().ok()?,
                month: month_number(month)?,
                day: day
                    .trim_end_matches(',')
                    .trim_end_matches(|c: char| c.is_ascii_alphabetic())
                    .parse()
                    .ok()?,
            };
            (date, *time, *meridiem)
        }
        _ => return None,
    };
    let (hour, minute) = clock_time.split_once(':')?;
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = minute.parse().ok()?;
    if !(1..=12).contains(&hour) || minute > 59 || !(1..=31).contains(&date.day) {
        return None;
    }
    let hour = match meridiem.to_ascii_uppercase().as_str() {
        "AM" => hour % 12,
        "PM" => hour % 12 + 12,
        _ => return None,
    };
    clock.to_utc(date, hour, minute)
}

/// Days from 1970-01-01 to a date, negative before it. Howard Hinnant's
/// algorithm for the proleptic Gregorian calendar.
pub fn days_from_civil(date: Date) -> i64 {
    let year = if date.month <= 2 {
        date.year - 1
    } else {
        date.year
    };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = i64::from((date.month + 9) % 12);
    let day_of_year = (153 * month_from_march + 2) / 5 + i64::from(date.day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The date `days` after 1970-01-01. The inverse of [`days_from_civil`].
#[cfg(test)]
pub fn civil_from_days(days: i64) -> Date {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    Date {
        year,
        month: u32::try_from(month).unwrap_or(1),
        day: u32::try_from(day).unwrap_or(1),
    }
}

/// A `YYYY-MM-DD` date.
fn parse_date(text: &str) -> Option<Date> {
    if text.len() != 10 || text.get(4..5) != Some("-") || text.get(7..8) != Some("-") {
        return None;
    }
    let date = Date {
        year: i64::from(digits(text, 0..4)?),
        month: digits(text, 5..7)?,
        day: digits(text, 8..10)?,
    };
    ((1..=12).contains(&date.month) && (1..=31).contains(&date.day)).then_some(date)
}

/// The number in `range` of `text`, when it's all ASCII digits.
fn digits(text: &str, range: std::ops::Range<usize>) -> Option<u32> {
    let part = text.get(range)?;
    if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

/// The month a name or its first three letters stands for, from 1.
fn month_number(name: &str) -> Option<u32> {
    /// The months' first three letters.
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let name = name.to_ascii_lowercase();
    let index = MONTHS.iter().position(|month| name.starts_with(month))?;
    u32::try_from(index + 1).ok()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A time zone a fixed number of minutes ahead of UTC, so tests don't
    /// depend on the machine's.
    pub struct FixedClock(pub i64);

    impl LocalClock for FixedClock {
        fn date_at(&self, at: Timestamp) -> Option<Date> {
            let local = i64::try_from(at.0 / 1000).ok()? + self.0 * 60;
            Some(civil_from_days(local.div_euclid(86_400)))
        }

        fn to_utc(&self, date: Date, hour: u32, minute: u32) -> Option<Timestamp> {
            let local = days_from_civil(date) * 86_400 + i64::from(hour * 3600 + minute * 60);
            let secs = u64::try_from(local - self.0 * 60).ok()?;
            Some(Timestamp(secs * 1000))
        }
    }

    #[test]
    fn rollout_times_parse_as_utc() {
        // 2026-10-19T20:11:26Z is 1792440686 seconds after the epoch.
        assert_eq!(
            parse_utc("2026-10-19T20:11:26.828Z"),
            Some(Timestamp(1_792_440_686_828))
        );
        assert_eq!(
            parse_utc("2026-10-19T20:11:26Z"),
            Some(Timestamp(1_792_440_686_000))
        );
        assert_eq!(
            parse_utc("2026-10-19T22:11:26.8+02:00"),
            Some(Timestamp(1_792_440_686_800))
        );
        assert_eq!(
            parse_utc("2026-10-19T20:11:26.828123456Z"),
            Some(Timestamp(1_792_440_686_828))
        );
        assert_eq!(parse_utc("2026-10-19"), None);
        assert_eq!(parse_utc("2026-13-19T20:11:26Z"), None);
        assert_eq!(parse_utc("yesterday at noon"), None);
        assert_eq!(parse_utc("2026-10-19T20:11:26+01é0"), None);
        assert_eq!(parse_utc("2026é-10-19T20:11:26Z"), None);
    }

    #[test]
    fn dates_round_trip_through_day_counts() {
        for days in [-719_468, -1, 0, 1, 59, 60, 20_744, 20_755, 2_932_896] {
            assert_eq!(days_from_civil(civil_from_days(days)), days);
        }
        let leap_day = Date {
            year: 2028,
            month: 2,
            day: 29,
        };
        assert_eq!(civil_from_days(days_from_civil(leap_day)), leap_day);
    }

    #[test]
    fn a_limit_message_gives_the_reset_time() {
        // Written at 2026-10-27 14:00 UTC, which is 10:00 in UTC-4.
        let written = parse_utc("2026-10-27T14:00:00Z").unwrap();
        let clock = FixedClock(-4 * 60);
        let same_day = "You've hit your usage limit. Upgrade to Pro \
             (https://example.com/pro), visit https://example.com/usage to \
             purchase more credits or try again at 3:05 PM.";
        assert_eq!(
            limit_resets_at(same_day, written, &clock),
            parse_utc("2026-10-27T19:05:00Z")
        );
        let later_day = "You've hit your usage limit. Try again at Nov 3rd, 2026 12:40 AM.";
        assert_eq!(
            limit_resets_at(later_day, written, &clock),
            parse_utc("2026-11-03T04:40:00Z")
        );
        let noon = "You've hit your usage limit. Try again at 12:00 PM.";
        assert_eq!(
            limit_resets_at(noon, written, &clock),
            parse_utc("2026-10-27T16:00:00Z")
        );
    }

    #[test]
    fn a_limit_message_without_a_time_gives_none() {
        let written = Timestamp(1_792_440_686_828);
        let clock = FixedClock(0);
        for message in [
            "You've hit your usage limit. Try again later.",
            "You've hit your usage limit. Try again at tea time.",
            "You've hit your usage limit. Try again at 13:05 PM.",
            "Try again at 3:0é PM.",
            "Try again at Oct 2éth, 2026 3:05 PM.",
            "",
        ] {
            assert_eq!(limit_resets_at(message, written, &clock), None, "{message}");
        }
    }

    #[test]
    fn the_system_clock_agrees_with_itself() {
        let clock = SystemClock;
        let at = parse_utc("2026-10-27T14:00:00Z").unwrap();
        let date = clock.date_at(at).expect("a local date");
        let noon = clock.to_utc(date, 12, 0).expect("local noon");
        assert_eq!(clock.date_at(noon), Some(date));
        // Every time zone is within a day of UTC.
        let gap = noon.0.abs_diff(at.0);
        assert!(gap < 2 * 86_400_000, "{gap}");
    }
}
