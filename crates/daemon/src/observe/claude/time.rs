// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! ISO 8601 times as Claude writes them, such as `2026-10-25T00:49:20.281Z`
//! in a transcript and `2026-10-25T02:29:59.941420+00:00` in a usage reply.

use maxhq_protocol::time::Timestamp;

/// Reads a date and time with a `Z` or `±HH:MM` offset. Returns none for
/// anything else, including a time with no offset, which would be local,
/// and a time before 1970.
pub fn parse_iso8601(text: &str) -> Option<Timestamp> {
    let text = text.trim();
    let (date, time) = text.split_at_checked(10)?;
    let time = time.strip_prefix(['T', 't', ' '])?;

    let mut parts = date.split('-');
    let year = number(parts.next()?, 4)?;
    let month = number(parts.next()?, 2)?;
    let day = number(parts.next()?, 2)?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let (clock, offset) = split_offset(time)?;
    let (hms, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    let mut parts = hms.split(':');
    let hour = number(parts.next()?, 2)?;
    let minute = number(parts.next()?, 2)?;
    let second = number(parts.next()?, 2)?;
    if parts.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let millis = fraction_millis(fraction)?;

    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset;
    let total = seconds.checked_mul(1_000)?.checked_add(millis)?;
    u64::try_from(total).ok().map(Timestamp)
}

/// Splits `HH:MM:SS.fff+01:00` into the clock and the offset in seconds.
fn split_offset(time: &str) -> Option<(&str, i64)> {
    if let Some(clock) = time.strip_suffix(['Z', 'z']) {
        return Some((clock, 0));
    }
    let at = time.rfind(['+', '-'])?;
    let (clock, offset) = time.split_at(at);
    let sign = if offset.starts_with('-') { -1 } else { 1 };
    let offset = &offset[1..];
    let (hours, minutes) = match offset.split_once(':') {
        Some((hours, minutes)) => (hours, minutes),
        None if offset.len() == 4 => offset.split_at(2),
        None => (offset, "00"),
    };
    let hours = number(hours, 2)?;
    let minutes = number(minutes, 2)?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some((clock, sign * (hours * 3_600 + minutes * 60)))
}

/// The milliseconds in a fraction of a second, such as `941420` for
/// 941 ms. Digits past the third are dropped.
fn fraction_millis(fraction: &str) -> Option<i64> {
    if !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let digits: String = fraction.chars().chain("000".chars()).take(3).collect();
    digits.parse().ok()
}

/// Parses exactly `len` ASCII digits.
fn number(text: &str, len: usize) -> Option<i64> {
    if text.len() != len || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Days from 1970-01-01 to a date in the proleptic Gregorian calendar,
/// after Howard Hinnant's `days_from_civil`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_transcript_and_usage_times() {
        assert_eq!(
            parse_iso8601("2026-10-25T00:49:20.281Z"),
            Some(Timestamp(1_792_889_360_281))
        );
        assert_eq!(
            parse_iso8601("2026-10-25T02:29:59.941420+00:00"),
            Some(Timestamp(1_792_895_399_941))
        );
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(Timestamp(0)));
    }

    #[test]
    fn applies_the_offset() {
        let utc = parse_iso8601("2026-03-01T12:00:00Z").unwrap();
        assert_eq!(parse_iso8601("2026-03-01T13:30:00+01:30"), Some(utc));
        assert_eq!(parse_iso8601("2026-03-01T07:00:00-0500"), Some(utc));
        assert_eq!(parse_iso8601("2026-03-01T14:00:00+02"), Some(utc));
    }

    #[test]
    fn leap_days_count() {
        let before = parse_iso8601("2028-02-28T00:00:00Z").unwrap();
        let after = parse_iso8601("2028-03-01T00:00:00Z").unwrap();
        assert_eq!(after.0 - before.0, 2 * 86_400_000);
    }

    #[test]
    fn rejects_anything_else() {
        for text in [
            "",
            "2026-10-25",
            "2026-10-25T00:49:20",
            "2026-13-25T00:49:20Z",
            "2026-10-25T24:00:00Z",
            "2026-10-25T00:49:20.2x1Z",
            "+2026-10-25T00:49:20Z",
            "1969-12-31T23:59:59Z",
            "yesterday",
        ] {
            assert_eq!(parse_iso8601(text), None, "{text}");
        }
    }
}
