// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Reads asciicast v2 and v3 recordings.
//!
//! Both are newline-delimited JSON: a header object, then one
//! `[time, code, data]` array per event. v2 times count from the start of the
//! recording and v3 times from the previous event. v3 also allows `#` comment
//! lines after the header. Unknown codes are skipped, as both specs ask. See
//! <https://docs.asciinema.org/manual/asciicast/v3/>.

use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

/// A parsed recording.
#[derive(Clone, Debug, PartialEq)]
pub struct Recording {
    /// 2 or 3.
    pub version: u8,
    /// Terminal width when recording started.
    pub cols: u16,
    /// Terminal height when recording started.
    pub rows: u16,
    /// Longest pause the recording asks a player to keep, if it sets one.
    pub idle_time_limit: Option<Duration>,
    /// Events in recording order.
    pub events: Vec<Event>,
}

/// One event, at its time since the recording started.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// Time since the recording started.
    pub at: Duration,
    /// What happened.
    pub kind: EventKind,
}

/// What an event records.
#[derive(Clone, Debug, PartialEq)]
pub enum EventKind {
    /// Bytes the program wrote, code `o`.
    Output(String),
    /// Keys typed, code `i`.
    Input(String),
    /// The terminal changed size, code `r`.
    Resize {
        /// New width.
        cols: u16,
        /// New height.
        rows: u16,
    },
    /// A bookmark, code `m`.
    Marker(String),
    /// The program exited with this status, code `x`. v3 only.
    Exit(i32),
}

impl Recording {
    /// Time of the last event.
    pub fn duration(&self) -> Duration {
        self.events.last().map_or(Duration::ZERO, |event| event.at)
    }

    /// Total bytes of output.
    pub fn output_bytes(&self) -> usize {
        self.events
            .iter()
            .map(|event| match &event.kind {
                EventKind::Output(data) => data.len(),
                _ => 0,
            })
            .sum()
    }
}

/// Why a recording didn't parse. Line numbers count from 1.
#[derive(Debug, Error)]
pub enum CastError {
    /// The file has no header line.
    #[error("the recording is empty")]
    Empty,
    /// The first line isn't a header.
    #[error("line 1 isn't an asciicast header: {0}")]
    Header(serde_json::Error),
    /// Only v2 and v3 are supported.
    #[error("asciicast v{0} isn't supported, only v2 and v3")]
    Version(u8),
    /// The header has no terminal size.
    #[error("the header doesn't give the terminal size")]
    NoSize,
    /// An event line isn't `[time, code, data]`.
    #[error("line {line} isn't an event: {source}")]
    Event {
        /// Where the bad line is.
        line: usize,
        /// What was wrong with it.
        source: serde_json::Error,
    },
    /// A time is negative, not a number, or puts the event further in than
    /// a `Duration` reaches.
    #[error("line {line} has a bad time, {time}")]
    Time {
        /// Where the bad line is.
        line: usize,
        /// The time as written.
        time: f64,
    },
    /// An `r` event isn't `COLSxROWS`, an `x` event isn't a status, or a
    /// code that needs text has something else.
    #[error("line {line} has bad {code:?} data, {data:?}")]
    Data {
        /// Where the bad line is.
        line: usize,
        /// The event code.
        code: String,
        /// The data as written.
        data: String,
    },
    /// The header's idle limit is longer than a `Duration` reaches.
    #[error("the header has a bad idle_time_limit, {0}")]
    IdleLimit(f64),
}

/// The codes this reader knows. Each one's data is a string.
const CODES: [&str; 5] = ["o", "i", "r", "m", "x"];

#[derive(Deserialize)]
struct Header {
    version: u8,
    // v2
    width: Option<u16>,
    height: Option<u16>,
    // v3
    term: Option<Term>,
    idle_time_limit: Option<f64>,
}

#[derive(Deserialize)]
struct Term {
    cols: u16,
    rows: u16,
}

/// Parses a whole recording.
pub fn parse(text: &str) -> Result<Recording, CastError> {
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line));
    let (_, first) = lines.next().ok_or(CastError::Empty)?;
    let header: Header = serde_json::from_str(first).map_err(CastError::Header)?;
    let (cols, rows) = match header.version {
        2 => header.width.zip(header.height),
        3 => header.term.map(|term| (term.cols, term.rows)),
        other => return Err(CastError::Version(other)),
    }
    .ok_or(CastError::NoSize)?;
    let relative = header.version == 3;
    let idle_time_limit = header
        .idle_time_limit
        .filter(|limit| *limit > 0.0)
        .map(|limit| Duration::try_from_secs_f64(limit).map_err(|_| CastError::IdleLimit(limit)))
        .transpose()?;

    let mut events = Vec::new();
    let mut clock = 0.0_f64;
    for (line, raw) in lines {
        let trimmed = raw.trim();
        if trimmed.is_empty() || (relative && trimmed.starts_with('#')) {
            continue;
        }
        let (time, code, data): (f64, String, serde_json::Value) =
            serde_json::from_str(trimmed).map_err(|source| CastError::Event { line, source })?;
        if !time.is_finite() || time < 0.0 {
            return Err(CastError::Time { line, time });
        }
        // A v2 time earlier than the one before it plays with no pause, so
        // the clock never runs backwards.
        clock = if relative {
            clock + time
        } else {
            clock.max(time)
        };
        let at = Duration::try_from_secs_f64(clock).map_err(|_| CastError::Time { line, time })?;
        let data = match data {
            serde_json::Value::String(data) => data,
            // An unknown code's data can be any JSON.
            _ if !CODES.contains(&code.as_str()) => continue,
            other => {
                return Err(CastError::Data {
                    line,
                    code,
                    data: other.to_string(),
                });
            }
        };
        let bad_data = || CastError::Data {
            line,
            code: code.clone(),
            data: data.clone(),
        };
        let kind = match code.as_str() {
            "o" => EventKind::Output(data),
            "i" => EventKind::Input(data),
            "m" => EventKind::Marker(data),
            "r" => {
                let (cols, rows) = data.split_once('x').ok_or_else(bad_data)?;
                let cols = cols.parse().map_err(|_| bad_data())?;
                let rows = rows.parse().map_err(|_| bad_data())?;
                EventKind::Resize { cols, rows }
            }
            "x" => EventKind::Exit(data.parse().map_err(|_| bad_data())?),
            _ => continue,
        };
        events.push(Event { at, kind });
    }

    Ok(Recording {
        version: header.version,
        cols,
        rows,
        idle_time_limit,
        events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(at_ms: u64, data: &str) -> Event {
        Event {
            at: Duration::from_millis(at_ms),
            kind: EventKind::Output(data.into()),
        }
    }

    #[test]
    fn reads_v2_with_absolute_times() {
        let text = concat!(
            r#"{"version": 2, "width": 120, "height": 40, "idle_time_limit": 2.5}"#,
            "\n",
            r#"[0.25, "o", "hello "]"#,
            "\n",
            r#"[1.5, "o", "world"]"#,
            "\n",
        );
        let recording = parse(text).unwrap();
        assert_eq!(
            (recording.version, recording.cols, recording.rows),
            (2, 120, 40)
        );
        assert_eq!(recording.idle_time_limit, Some(Duration::from_millis(2500)));
        assert_eq!(
            recording.events,
            [output(250, "hello "), output(1500, "world")]
        );
        assert_eq!(recording.output_bytes(), 11);
    }

    #[test]
    fn reads_v3_with_intervals_comments_and_exit() {
        let text = concat!(
            r#"{"version": 3, "term": {"cols": 200, "rows": 50, "type": "xterm-256color"}}"#,
            "\n",
            "# a comment\n",
            r#"[0.5, "o", "a"]"#,
            "\n\n",
            r#"[0.25, "r", "100x30"]"#,
            "\n",
            r#"[0.25, "x", "3"]"#,
            "\n",
        );
        let recording = parse(text).unwrap();
        assert_eq!((recording.cols, recording.rows), (200, 50));
        assert_eq!(
            recording.events,
            [
                output(500, "a"),
                Event {
                    at: Duration::from_millis(750),
                    kind: EventKind::Resize {
                        cols: 100,
                        rows: 30
                    }
                },
                Event {
                    at: Duration::from_millis(1000),
                    kind: EventKind::Exit(3)
                },
            ]
        );
        assert_eq!(recording.duration(), Duration::from_secs(1));
    }

    #[test]
    fn skips_unknown_codes() {
        let text = "{\"version\": 3, \"term\": {\"cols\": 80, \"rows\": 24}}\n[0.1, \"z\", \"?\"]\n[0.1, \"z\", {\"k\": 1}]\n[0.1, \"o\", \"b\"]\n";
        assert_eq!(parse(text).unwrap().events, [output(300, "b")]);
    }

    #[test]
    fn v2_times_never_go_backwards() {
        let header = "{\"version\": 2, \"width\": 80, \"height\": 24}\n";
        let events =
            "[0, \"o\", \"a\"]\n[10, \"o\", \"b\"]\n[5, \"o\", \"c\"]\n[11, \"o\", \"d\"]\n";
        let recording = parse(&format!("{header}{events}")).unwrap();
        assert_eq!(
            recording.events,
            [
                output(0, "a"),
                output(10_000, "b"),
                output(10_000, "c"),
                output(11_000, "d")
            ]
        );
        assert_eq!(recording.duration(), Duration::from_secs(11));
    }

    #[test]
    fn rejects_what_it_cant_play() {
        assert!(matches!(parse(""), Err(CastError::Empty)));
        assert!(matches!(
            parse("{\"version\": 1}"),
            Err(CastError::Version(1))
        ));
        assert!(matches!(parse("{\"version\": 2}"), Err(CastError::NoSize)));
        let header = "{\"version\": 2, \"width\": 80, \"height\": 24}\n";
        assert!(matches!(
            parse(&format!("{header}[-1, \"o\", \"\"]")),
            Err(CastError::Time { line: 2, .. })
        ));
        assert!(matches!(
            parse(&format!("{header}[0, \"r\", \"wide\"]")),
            Err(CastError::Data { line: 2, .. })
        ));
        assert!(matches!(
            parse(&format!("{header}# v2 has no comments")),
            Err(CastError::Event { line: 2, .. })
        ));
        assert!(matches!(
            parse(&format!("{header}[0, \"o\", 5]")),
            Err(CastError::Data { line: 2, .. })
        ));
    }

    #[test]
    fn rejects_times_a_duration_cant_hold() {
        let v2 = "{\"version\": 2, \"width\": 80, \"height\": 24}\n[1e20, \"o\", \"x\"]\n";
        assert!(matches!(parse(v2), Err(CastError::Time { line: 2, .. })));
        // Each interval is fine on its own, but together they overflow.
        let v3 = "{\"version\": 3, \"term\": {\"cols\": 80, \"rows\": 24}}\n[1e19, \"o\", \"x\"]\n[1e19, \"o\", \"x\"]\n";
        assert!(matches!(parse(v3), Err(CastError::Time { line: 3, .. })));
        let limit = "{\"version\": 2, \"width\": 80, \"height\": 24, \"idle_time_limit\": 1e20}\n";
        assert!(matches!(parse(limit), Err(CastError::IdleLimit(_))));
    }
}
