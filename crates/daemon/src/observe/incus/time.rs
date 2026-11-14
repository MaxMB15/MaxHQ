// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Reads the times Incus and the workspace hooks write.

use maxhq_protocol::time::Timestamp;

/// Reads an RFC 3339 time, such as `2026-10-23T04:03:13.744621176Z` from
/// Incus or `2026-10-24T09:01:03+00:00` from `date -Is`, and the form Go
/// prints in headers, such as `2026-10-24 09:01:03 +0000 UTC`. Anything after
/// the offset is ignored. None for anything else, and for times at or before
/// 1970, which Incus uses for "never".
pub fn parse(text: &str) -> Option<Timestamp> {
    let b = text.trim().as_bytes();
    if b.len() < 20 || !b"Tt ".contains(&b[10]) {
        return None;
    }
    for (at, sep) in [(4, b'-'), (7, b'-'), (13, b':'), (16, b':')] {
        if b[at] != sep {
            return None;
        }
    }
    let year = number(b, 0, 4)?;
    let month = number(b, 5, 2)?;
    let day = number(b, 8, 2)?;
    let hour = number(b, 11, 2)?;
    let minute = number(b, 14, 2)?;
    // A leap second reads as the second before it.
    let second = number(b, 17, 2)?.min(59);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }

    let mut at = 19;
    let mut millis = 0;
    if b[at] == b'.' {
        at += 1;
        let start = at;
        while b.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        if at == start {
            return None;
        }
        // Only the first three digits count. Pad short fractions to three.
        millis = b[start..at]
            .iter()
            .chain(b"00")
            .take(3)
            .fold(0, |n, d| n * 10 + i64::from(d - b'0'));
    }
    if b.get(at) == Some(&b' ') {
        at += 1;
    }
    let offset = match b.get(at)? {
        b'Z' | b'z' => 0,
        sign @ (b'+' | b'-') => {
            let hours = number(b, at + 1, 2)?;
            let minutes_at = if b.get(at + 3) == Some(&b':') {
                at + 4
            } else {
                at + 3
            };
            let minutes = number(b, minutes_at, 2)?;
            let offset = hours * 60 + minutes;
            if *sign == b'-' {
                -offset
            } else {
                offset
            }
        }
        _ => return None,
    };

    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset * 60;
    let millis = seconds.checked_mul(1_000)?.checked_add(millis)?;
    u64::try_from(millis)
        .ok()
        .filter(|&ms| ms > 0)
        .map(Timestamp)
}

/// The decimal number in `len` digits at `at`.
fn number(b: &[u8], at: usize, len: usize) -> Option<i64> {
    let digits = b.get(at..at + len)?;
    digits.iter().try_fold(0, |n, d| {
        d.is_ascii_digit().then(|| n * 10 + i64::from(d - b'0'))
    })
}

/// Days from 1970-01-01 to a date in the proleptic Gregorian calendar, after
/// Howard Hinnant's `days_from_civil`.
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
    fn reads_the_times_incus_and_the_hooks_write() {
        let cases = [
            // Incus, in UTC with nanoseconds.
            ("2026-10-23T04:03:13.744621176Z", 1_792_728_193_744),
            // The hook's `date -Is`.
            ("2026-10-24T09:01:03+00:00", 1_792_832_463_000),
            ("2026-10-18T02:34:46+02:00", 1_792_283_686_000),
            ("2026-02-28T23:59:59-05:30", 1_772_342_999_000),
            // An event on a host an hour east of UTC.
            ("2026-10-25T02:16:21.819893374+01:00", 1_792_890_981_819),
            // The X-Incus-Modified header.
            ("2026-10-24 09:01:03 +0000 UTC", 1_792_832_463_000),
            ("2000-03-01T00:00:00Z", 951_868_800_000),
        ];
        for (text, millis) in cases {
            assert_eq!(parse(text), Some(Timestamp(millis)), "{text}");
        }
    }

    #[test]
    fn short_fractions_are_padded() {
        assert_eq!(
            parse("2026-10-24T09:01:03.5Z"),
            Some(Timestamp(1_792_832_463_500))
        );
    }

    #[test]
    fn never_and_nonsense_read_as_none() {
        for text in [
            "0001-01-01T00:00:00Z",
            "1970-01-01T00:00:00Z",
            "",
            "yesterday",
            "2026-10-24T09:01:03",
            "2026-13-24T09:01:03Z",
            "2026-10-24T09:01:03.Z",
            "2026-10-24T09:01:03+0",
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
    }
}
