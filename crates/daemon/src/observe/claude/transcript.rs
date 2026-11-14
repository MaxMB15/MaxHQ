// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The last assistant reply in a Claude Code transcript. A transcript can
//! pass a hundred megabytes, so a [`Tail`] reads each one once from near its
//! end and after that only what's been added.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use maxhq_protocol::model::ContextUsage;
use maxhq_protocol::time::Timestamp;
use serde::Deserialize;

use super::time::parse_iso8601;

/// How long the prompt cache lasts after a write that used the 5-minute TTL.
pub const FIVE_MINUTES: Duration = Duration::from_secs(5 * 60);

/// How long it lasts after a write that used the 1-hour TTL, which Claude
/// Code uses for the main conversation on a subscription within its limits.
pub const ONE_HOUR: Duration = Duration::from_secs(60 * 60);

/// The usual context window.
pub const WINDOW: u32 = 200_000;

/// The window of a model whose ID ends in `[1m]`.
pub const LONG_WINDOW: u32 = 1_000_000;

/// How much of the end to read first.
const FIRST_READ: u64 = 4 << 20;

/// How far back to look before giving up on finding a reply.
const LAST_READ: u64 = 64 << 20;

/// What the transcript says about its last reply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LastTurn {
    /// When the last assistant line was written. For an idle session that's
    /// when its last turn ended.
    pub ended_at: Option<Timestamp>,
    /// The context in use after the last reply that reported usage.
    pub context: Option<ContextUsage>,
    /// The TTL of the last reply that wrote to the prompt cache.
    pub cache_ttl: Option<Duration>,
}

impl LastTurn {
    /// When the prompt cache from the last turn expires, if there was one.
    /// A transcript that never says which TTL it used gets the shorter one.
    pub fn warm_until(&self) -> Option<Timestamp> {
        Some(self.ended_at?.after(self.cache_ttl.unwrap_or(FIVE_MINUTES)))
    }

    /// Updates this from one transcript line. `lead` skips sidechain lines,
    /// which older versions wrote into the lead's transcript for its
    /// sub-agents.
    pub fn read_line(&mut self, line: &[u8], lead: bool) {
        // Most lines are user messages, tool results and bookkeeping, so
        // skip anything that can't be an assistant line before parsing.
        if !contains(line, br#""assistant""#) {
            return;
        }
        let Ok(line) = serde_json::from_slice::<Line>(line) else {
            return;
        };
        if line.kind.as_deref() != Some("assistant") || (lead && line.is_sidechain == Some(true)) {
            return;
        }
        if let Some(at) = line.timestamp.as_deref().and_then(parse_iso8601) {
            self.ended_at = Some(at);
        }
        let Some(message) = line.message else {
            return;
        };
        // Claude Code writes `<synthetic>` replies itself, such as for an
        // interrupted turn, with no real usage.
        if message.model.as_deref() == Some("<synthetic>") {
            return;
        }
        let Some(usage) = message.usage else {
            return;
        };
        let used = [
            usage.input_tokens,
            usage.cache_read_input_tokens,
            usage.cache_creation_input_tokens,
        ]
        .into_iter()
        .flatten()
        .fold(0u64, u64::saturating_add);
        let long = [&line.requested_model, &message.model]
            .into_iter()
            .flatten()
            .any(|model| model.ends_with("[1m]"));
        // A window can't hold more than its size, so a bigger count means
        // the long window even when the model ID doesn't say.
        let window = if long || used > u64::from(WINDOW) {
            LONG_WINDOW
        } else {
            WINDOW
        };
        self.context = Some(ContextUsage {
            used_tokens: u32::try_from(used).unwrap_or(u32::MAX),
            window_tokens: window,
        });
        if let Some(ttl) = usage.ttl() {
            self.cache_ttl = Some(ttl);
        }
    }
}

/// The parts of a transcript line this reads.
#[derive(Deserialize)]
struct Line {
    /// `assistant` for a reply.
    #[serde(rename = "type")]
    kind: Option<String>,
    /// Set on lines from a sub-agent.
    #[serde(rename = "isSidechain")]
    is_sidechain: Option<bool>,
    /// When the line was written.
    timestamp: Option<String>,
    /// The model as asked for, which carries the `[1m]` suffix when set.
    #[serde(rename = "requestedModel")]
    requested_model: Option<String>,
    /// The API's reply.
    message: Option<Message>,
}

/// The parts of a reply this reads.
#[derive(Deserialize)]
struct Message {
    /// The model that answered.
    model: Option<String>,
    /// Token counts for the request.
    usage: Option<Usage>,
}

/// A reply's token counts.
#[derive(Deserialize)]
struct Usage {
    /// Tokens sent outside the cache.
    input_tokens: Option<u64>,
    /// Tokens read from the cache.
    cache_read_input_tokens: Option<u64>,
    /// Tokens written to the cache.
    cache_creation_input_tokens: Option<u64>,
    /// The cache writes by TTL.
    cache_creation: Option<CacheCreation>,
}

/// Cache writes split by TTL.
#[derive(Deserialize)]
struct CacheCreation {
    /// Tokens written with the 1-hour TTL.
    ephemeral_1h_input_tokens: Option<u64>,
    /// Tokens written with the 5-minute TTL.
    ephemeral_5m_input_tokens: Option<u64>,
}

impl Usage {
    /// The TTL this reply wrote to the cache with, or none when it only
    /// read from the cache, which keeps the TTL it already had.
    fn ttl(&self) -> Option<Duration> {
        let positive = |count: Option<u64>| count.is_some_and(|count| count > 0);
        match &self.cache_creation {
            Some(split) if positive(split.ephemeral_1h_input_tokens) => Some(ONE_HOUR),
            Some(split) if positive(split.ephemeral_5m_input_tokens) => Some(FIVE_MINUTES),
            Some(_) => None,
            None => positive(self.cache_creation_input_tokens).then_some(FIVE_MINUTES),
        }
    }
}

/// Follows one transcript as it grows.
#[derive(Debug)]
pub struct Tail {
    /// The transcript.
    path: PathBuf,
    /// Whether this is a lead's transcript rather than a sub-agent's.
    lead: bool,
    /// Where the next unread line starts, or none before the first read.
    offset: Option<u64>,
    /// What the lines read so far say.
    last: LastTurn,
}

impl Tail {
    /// Starts following `path`, which needn't exist yet.
    pub fn new(path: PathBuf, lead: bool) -> Self {
        Self {
            path,
            lead,
            offset: None,
            last: LastTurn::default(),
        }
    }

    /// The transcript this follows.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads whatever was added since the last call. A file that shrank was
    /// rewritten, so it's read again from near the end.
    pub fn poll(&mut self) -> io::Result<&LastTurn> {
        let mut file = File::open(&self.path)?;
        let len = file.metadata()?.len();
        match self.offset {
            Some(offset) if offset <= len => {
                if offset < len {
                    let end = read_lines(&mut file, offset, len, false, self.lead, &mut self.last)?;
                    self.offset = Some(end);
                }
            }
            _ => {
                let (end, last) = read_end(&mut file, len, self.lead)?;
                self.offset = Some(end);
                self.last = last;
            }
        }
        Ok(&self.last)
    }
}

/// Reads the end of a transcript, going further back until it finds a
/// reply or reaches [`LAST_READ`]. Returns where the read stopped.
fn read_end(file: &mut File, len: u64, lead: bool) -> io::Result<(u64, LastTurn)> {
    let mut size = FIRST_READ;
    loop {
        let start = len.saturating_sub(size);
        let mut last = LastTurn::default();
        let end = read_lines(file, start, len, start > 0, lead, &mut last)?;
        if last.ended_at.is_some() || start == 0 || size >= LAST_READ {
            return Ok((end, last));
        }
        size *= 4;
    }
}

/// Reads the whole lines between `start` and `len` into `last`. Returns
/// where the last whole line ends, so a line still being written is read
/// next time. `skip_first` drops the first line, which starts before
/// `start` when reading from partway in.
fn read_lines(
    file: &mut File,
    start: u64,
    len: u64,
    skip_first: bool,
    lead: bool,
    last: &mut LastTurn,
) -> io::Result<u64> {
    file.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::with_capacity(1 << 16, io::Read::take(&mut *file, len - start));
    let mut line = Vec::new();
    let mut end = start;
    let mut skip = skip_first;
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 || line.last() != Some(&b'\n') {
            return Ok(end);
        }
        end += read as u64;
        if !std::mem::take(&mut skip) {
            last.read_line(&line, lead);
        }
    }
}

/// Whether `needle` appears in `haystack`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;

    use super::*;
    use crate::observe::claude::tests::{fixture, scratch};

    const LEAD: &str = "config/projects/-work-maxhq/6f1d2c3b-0a4e-4b8f-9c11-2d3e4f5a6b7c.jsonl";

    #[test]
    fn reads_the_last_reply() {
        let mut tail = Tail::new(fixture(LEAD), true);
        let last = tail.poll().unwrap().clone();
        assert_eq!(last.ended_at, parse_iso8601("2026-10-25T00:49:20.281Z"));
        assert_eq!(
            last.context,
            Some(ContextUsage {
                used_tokens: 6 + 81_250 + 1_874,
                window_tokens: WINDOW,
            })
        );
        assert_eq!(last.cache_ttl, Some(ONE_HOUR));
        assert_eq!(last.warm_until(), parse_iso8601("2026-10-25T01:49:20.281Z"));
    }

    #[test]
    fn sidechain_and_synthetic_lines_keep_the_lead_reply() {
        let mut last = LastTurn::default();
        last.read_line(
            br#"{"type":"assistant","timestamp":"2026-10-25T00:00:00Z","message":{"model":"claude-opus-5-5","usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":30,"cache_creation":{"ephemeral_5m_input_tokens":30,"ephemeral_1h_input_tokens":0}}}}"#,
            true,
        );
        let first = last.clone();
        last.read_line(
            br#"{"type":"assistant","isSidechain":true,"timestamp":"2026-10-25T00:01:00Z","message":{"model":"claude-opus-5-5","usage":{"input_tokens":99}}}"#,
            true,
        );
        assert_eq!(last, first, "a sidechain line isn't the lead's reply");

        last.read_line(
            br#"{"type":"assistant","timestamp":"2026-10-25T00:02:00Z","message":{"model":"<synthetic>","usage":{"input_tokens":0}}}"#,
            true,
        );
        assert_eq!(last.ended_at, parse_iso8601("2026-10-25T00:02:00Z"));
        assert_eq!(last.context, first.context);
        assert_eq!(last.cache_ttl, Some(FIVE_MINUTES));

        last.read_line(
            br#"{"type":"user","message":{"content":"assistant"}}"#,
            true,
        );
        last.read_line(b"not json \"assistant\"", true);
        assert_eq!(last.ended_at, parse_iso8601("2026-10-25T00:02:00Z"));
    }

    #[test]
    fn the_window_follows_the_model() {
        let mut last = LastTurn::default();
        last.read_line(
            br#"{"type":"assistant","requestedModel":"claude-opus-5-5[1m]","timestamp":"2026-10-25T00:00:00Z","message":{"model":"claude-opus-5-5","usage":{"input_tokens":1000}}}"#,
            true,
        );
        assert_eq!(last.context.unwrap().window_tokens, LONG_WINDOW);

        last.read_line(
            br#"{"type":"assistant","timestamp":"2026-10-25T00:00:00Z","message":{"model":"claude-opus-5-5","usage":{"input_tokens":1000}}}"#,
            true,
        );
        assert_eq!(last.context.unwrap().window_tokens, WINDOW);

        last.read_line(
            br#"{"type":"assistant","timestamp":"2026-10-25T00:00:00Z","message":{"model":"claude-opus-5-5","usage":{"input_tokens":5,"cache_read_input_tokens":250000}}}"#,
            true,
        );
        assert_eq!(
            last.context,
            Some(ContextUsage {
                used_tokens: 250_005,
                window_tokens: LONG_WINDOW,
            })
        );
    }

    #[test]
    fn reads_only_what_was_added() {
        let dir = scratch();
        let path = dir.join("t.jsonl");
        fs::copy(fixture(LEAD), &path).unwrap();
        let mut tail = Tail::new(path.clone(), true);
        let first = tail.poll().unwrap().ended_at;

        // Half a line: Claude Code is still writing it.
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(br#"{"type":"assistant","timestamp":"2026-10-25T00:59:00Z","message":{"model":"claude-opus-5-5","usage":{"input_tokens":1}}"#)
            .unwrap();
        assert_eq!(tail.poll().unwrap().ended_at, first);

        file.write_all(b"}\n").unwrap();
        let last = tail.poll().unwrap();
        assert_eq!(last.ended_at, parse_iso8601("2026-10-25T00:59:00Z"));
        assert_eq!(last.context.unwrap().used_tokens, 1);
        assert_eq!(last.cache_ttl, Some(ONE_HOUR), "a read keeps the TTL");

        // Rewritten shorter: read again from the end.
        fs::write(&path, b"{\"type\":\"user\"}\n").unwrap();
        assert_eq!(tail.poll().unwrap(), &LastTurn::default());
    }

    #[test]
    fn finds_a_reply_far_from_the_end() {
        let dir = scratch();
        let path = dir.join("long.jsonl");
        let mut file = File::create(&path).unwrap();
        file.write_all(br#"{"type":"assistant","timestamp":"2026-10-25T00:00:00Z","message":{"model":"m","usage":{"input_tokens":7}}}"#)
            .unwrap();
        file.write_all(b"\n").unwrap();
        let filler = format!(
            "{{\"type\":\"progress\",\"data\":\"{}\"}}\n",
            "x".repeat(1 << 16)
        );
        for _ in 0..(FIRST_READ / filler.len() as u64 + 8) {
            file.write_all(filler.as_bytes()).unwrap();
        }
        drop(file);
        let mut tail = Tail::new(path, true);
        assert_eq!(
            tail.poll().unwrap().ended_at,
            parse_iso8601("2026-10-25T00:00:00Z")
        );
    }
}
