// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The run's history: one reader for both kinds of event log, read by
//! offset, plus the history the engine keeps itself when a run has no log.

use std::path::{Path, PathBuf};

use maxhq_protocol::ids::AgentId;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::workflow::NodeStatus;
use regex::Regex;
use serde_json::Value as Json;

use crate::descriptor::Nodes;
use crate::inputs::Files;
use crate::reference::EventLine;

/// The most bytes read from a log at once.
const CHUNK: usize = 1024 * 1024;
/// The most events kept per run. Older ones drop first.
const MAX_EVENTS: usize = 20_000;

/// One thing that happened to a node.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// When, never earlier than the event before it.
    pub t: Timestamp,
    /// The node, by id.
    pub node: String,
    /// Its new status, in the standard's words.
    pub status: Option<NodeStatus>,
    /// A note on it.
    pub note: Option<String>,
    /// The workflow's attempt number.
    pub attempt: Option<u64>,
    /// The agent that made it, when the workflow says.
    pub agent: Option<AgentId>,
}

/// How a log's lines read.
#[derive(Clone, Debug)]
pub(crate) enum Format {
    /// One JSON object per line.
    Jsonl,
    /// Named groups `t`, `node`, `status` and `note` of a pattern.
    Lines(Regex),
}

/// A log being read by offset.
#[derive(Debug)]
pub(crate) struct Log {
    pub path: PathBuf,
    format: Format,
    offset: u64,
}

impl Log {
    pub fn new(path: PathBuf, format: Format) -> Self {
        Self {
            path,
            format,
            offset: 0,
        }
    }

    /// Reads whatever was appended since the last call into `history`. A
    /// line with no newline yet waits for the next call. A log that shrank
    /// was replaced, so the history starts over.
    pub fn poll(&mut self, files: &dyn Files, nodes: &Nodes, history: &mut History) {
        let Ok(stat) = files.stat(&self.path) else {
            return;
        };
        if stat.size < self.offset {
            self.offset = 0;
            history.clear();
        }
        while stat.size > self.offset {
            let Ok(bytes) = files.read_from(&self.path, self.offset, CHUNK) else {
                return;
            };
            let Some(end) = bytes.iter().rposition(|b| *b == b'\n') else {
                // A line longer than a chunk can't be an event; skip it.
                if bytes.len() >= CHUNK {
                    self.offset += bytes.len() as u64;
                    continue;
                }
                return;
            };
            for line in bytes[..end].split(|b| *b == b'\n') {
                if let Some(event) = std::str::from_utf8(line)
                    .ok()
                    .and_then(|line| self.parse(line.trim_end_matches('\r'), nodes))
                {
                    history.push(event);
                }
            }
            self.offset += end as u64 + 1;
            if bytes.len() < CHUNK {
                return;
            }
        }
    }

    fn parse(&self, line: &str, nodes: &Nodes) -> Option<Event> {
        let (t, node, word, note, attempt, agent) = match &self.format {
            Format::Jsonl => {
                let e: EventLine = serde_json::from_str(line).ok()?;
                (e.t, e.node, e.status, e.note, e.attempt, e.agent)
            }
            Format::Lines(regex) => {
                let caps = regex.captures(line)?;
                let group = |name| caps.name(name).map(|m| m.as_str().to_owned());
                (
                    group("t")?,
                    group("node")?,
                    group("status"),
                    group("note"),
                    None,
                    None,
                )
            }
        };
        if word.is_none() && note.is_none() {
            return None;
        }
        Some(Event {
            t: parse_time(&t)?,
            node,
            status: word.map(|w| map_status(nodes, &w)),
            note,
            attempt,
            agent: agent.and_then(|a| a.parse().ok()),
        })
    }
}

/// Reads RFC 3339 times, and times with no offset as UTC.
pub(crate) fn parse_time(text: &str) -> Option<Timestamp> {
    let text = text.trim();
    let ts = text.parse::<jiff::Timestamp>().ok().or_else(|| {
        let civil: jiff::civil::DateTime = text.parse().ok()?;
        civil
            .to_zoned(jiff::tz::TimeZone::UTC)
            .ok()
            .map(|z| z.timestamp())
    })?;
    Some(Timestamp(u64::try_from(ts.as_millisecond()).unwrap_or(0)))
}

/// A workflow's status word in the standard's words.
pub(crate) fn map_status(nodes: &Nodes, word: &str) -> NodeStatus {
    if let Some(status) = nodes.status_map.get(word) {
        return *status;
    }
    serde_json::from_value(Json::from(word)).unwrap_or(NodeStatus::Unknown)
}

/// Events in order, with times clamped so they never run backwards.
#[derive(Debug, Default)]
pub(crate) struct History {
    events: Vec<Event>,
}

impl History {
    pub fn push(&mut self, mut event: Event) {
        if let Some(last) = self.events.last() {
            event.t = event.t.max(last.t);
        }
        if self.events.len() >= MAX_EVENTS {
            self.events.drain(..MAX_EVENTS / 10);
        }
        self.events.push(event);
    }

    pub fn clear(&mut self) {
        self.events.clear();
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn last_time(&self) -> Option<Timestamp> {
        self.events.last().map(|e| e.t)
    }
}

/// Which log a run reads: the first of the descriptor's that exists.
pub(crate) fn first_existing(files: &dyn Files, logs: Vec<(PathBuf, Format)>) -> Option<Log> {
    logs.into_iter()
        .find(|(path, _)| files.stat(Path::new(path)).is_ok())
        .map(|(path, format)| Log::new(path, format))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::MemFiles;

    const LOG: &str = "/p/run/events.jsonl";

    fn nodes() -> Nodes {
        let mut nodes = Nodes::default();
        nodes.status_map.0 = vec![
            ("in_progress".into(), NodeStatus::Busy),
            ("completed".into(), NodeStatus::Done),
        ];
        nodes
    }

    #[test]
    fn jsonl_reads_by_offset_and_waits_for_a_torn_line() {
        let mut files = MemFiles::default();
        files.put(
            LOG,
            "{\"t\":\"2026-11-01T14:02:07.120Z\",\"node\":\"a\",\"status\":\"in_progress\",\"attempt\":1,\"tier\":\"M\"}\n{\"t\":\"2026-11-01T14:03:00Z\",\"node\":\"a\",\"sta",
        );
        let mut log = Log::new(LOG.into(), Format::Jsonl);
        let mut history = History::default();
        log.poll(&files, &nodes(), &mut history);
        assert_eq!(history.events().len(), 1);
        let first = &history.events()[0];
        assert_eq!(first.status, Some(NodeStatus::Busy));
        assert_eq!(first.attempt, Some(1));
        assert_eq!(first.t, Timestamp(1_793_541_727_120));

        files.append(LOG, "tus\":\"completed\"}\n{\"t\":\"2026-11-01T14:04:00Z\",\"node\":\"b\",\"note\":\"dispatch refused\"}\n");
        let reads = files.reads();
        log.poll(&files, &nodes(), &mut history);
        assert_eq!(files.reads(), reads + 1);
        let events = history.events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[1].status, Some(NodeStatus::Done));
        assert_eq!(events[2].note.as_deref(), Some("dispatch refused"));
        assert_eq!(events[2].status, None);

        log.poll(&files, &nodes(), &mut history);
        assert_eq!(files.reads(), reads + 1, "nothing new reads nothing");
    }

    #[test]
    fn lines_without_status_or_note_and_bad_lines_are_skipped() {
        let mut files = MemFiles::default();
        files.put(
            LOG,
            "not json\n{\"t\":\"2026-11-01T14:02:07Z\",\"node\":\"a\"}\n{\"node\":\"a\",\"status\":\"x\"}\n{\"t\":\"2026-11-01T14:02:07Z\",\"node\":\"a\",\"status\":\"weird\"}\n",
        );
        let mut log = Log::new(LOG.into(), Format::Jsonl);
        let mut history = History::default();
        log.poll(&files, &nodes(), &mut history);
        assert_eq!(history.events().len(), 1);
        assert_eq!(history.events()[0].status, Some(NodeStatus::Unknown));
    }

    #[test]
    fn line_logs_clamp_backward_times() {
        let mut files = MemFiles::default();
        let journal = "/p/run/journal.md";
        files.put(
            journal,
            "# Journal\n- 2026-08-13T10:00:05Z build: in_progress.\n- 2026-08-13T09:59:00Z build: completed.\n- 2026-08-13T10:01:00 test: in_progress.\n",
        );
        let regex = Regex::new(r"^- (?<t>\S+) (?<node>[\w-]+): (?<status>\w+)\.$").unwrap();
        let mut log = Log::new(journal.into(), Format::Lines(regex));
        let mut history = History::default();
        log.poll(&files, &nodes(), &mut history);
        let events = history.events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[1].t, events[0].t, "clamped to the event before");
        assert_eq!(events[1].status, Some(NodeStatus::Done));
        assert!(events[2].t > events[1].t, "no offset reads as UTC");
    }

    #[test]
    fn a_log_that_shrank_starts_over() {
        let mut files = MemFiles::default();
        files.put(LOG, "{\"t\":\"2026-11-01T14:02:07Z\",\"node\":\"a\",\"status\":\"busy\"}\n{\"t\":\"2026-11-01T14:02:08Z\",\"node\":\"a\",\"status\":\"done\"}\n");
        let mut log = Log::new(LOG.into(), Format::Jsonl);
        let mut history = History::default();
        log.poll(&files, &nodes(), &mut history);
        assert_eq!(history.events().len(), 2);
        files.put(
            LOG,
            "{\"t\":\"2026-11-01T15:00:00Z\",\"node\":\"b\",\"status\":\"busy\"}\n",
        );
        log.poll(&files, &nodes(), &mut history);
        assert_eq!(history.events().len(), 1);
        assert_eq!(history.events()[0].node, "b");
    }

    #[test]
    fn the_first_log_that_exists_is_read() {
        let mut files = MemFiles::default();
        files.put("/p/run/journal.md", "");
        let regex = Regex::new("x").unwrap();
        let log = first_existing(
            &files,
            vec![
                (LOG.into(), Format::Jsonl),
                ("/p/run/journal.md".into(), Format::Lines(regex)),
            ],
        )
        .unwrap();
        assert_eq!(log.path, Path::new("/p/run/journal.md"));
    }
}
