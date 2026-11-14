// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The output log: one JSON line per line the CLI prints, on disk and as a
//! tail in memory.
//!
//! The holder appends each line with one write, so a reader sees whole
//! lines plus at most a partial last one, which it skips. When the log
//! reaches its limit it's renamed to `events.log.1`, replacing the one
//! before, and a new log starts.

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use maxhq_protocol::time::Timestamp;
use serde::{Deserialize, Serialize};

use crate::files::{self, EVENTS, EVENTS_OLD};
use crate::Line;

/// One recorded line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub seq: u64,
    pub at: Timestamp,
    pub text: Arc<str>,
}

impl Entry {
    /// The line as a client sees it.
    pub fn line(&self, replayed: bool) -> Line {
        Line {
            seq: self.seq,
            at: self.at,
            text: self.text.to_string(),
            replayed,
        }
    }
}

#[derive(Serialize)]
struct Written<'a> {
    seq: u64,
    at: Timestamp,
    line: &'a str,
}

#[derive(Deserialize)]
struct Stored {
    seq: u64,
    at: Timestamp,
    line: String,
}

/// The holder's side of the log.
pub(crate) struct EventLog {
    dir: PathBuf,
    file: File,
    size: u64,
    rotate_at: u64,
    first_current: Option<u64>,
    first_old: Option<u64>,
    last_seq: u64,
}

impl EventLog {
    /// Starts an empty log in `dir`.
    pub fn create(dir: &Path, rotate_at: u64) -> io::Result<Self> {
        let file = files::append(&dir.join(EVENTS))?;
        file.set_len(0)?;
        Ok(Self {
            dir: dir.to_owned(),
            file,
            size: 0,
            rotate_at: rotate_at.max(1),
            first_current: None,
            first_old: None,
            last_seq: 0,
        })
    }

    /// Records `text` as the next line and returns it. A line that can't be
    /// written still gets its number, so the numbering never repeats, and
    /// the error goes to the holder's log.
    pub fn append(&mut self, at: Timestamp, text: Arc<str>) -> Entry {
        self.last_seq += 1;
        let entry = Entry {
            seq: self.last_seq,
            at,
            text,
        };
        if let Err(err) = self.write(&entry) {
            eprintln!("couldn't record line {}: {err}", entry.seq);
        }
        entry
    }

    fn write(&mut self, entry: &Entry) -> io::Result<()> {
        let record = Written {
            seq: entry.seq,
            at: entry.at,
            line: &entry.text,
        };
        let mut bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
        bytes.push(b'\n');
        self.file.write_all(&bytes)?;
        self.size += bytes.len() as u64;
        self.first_current.get_or_insert(entry.seq);
        if self.size >= self.rotate_at {
            self.rotate()?;
        }
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        fs::rename(self.dir.join(EVENTS), self.dir.join(EVENTS_OLD))?;
        self.file = files::append(&self.dir.join(EVENTS))?;
        self.size = 0;
        self.first_old = self.first_current.take();
        Ok(())
    }

    /// The oldest line on disk, or the next line when there's none.
    pub fn first_seq(&self) -> u64 {
        self.first_old
            .or(self.first_current)
            .unwrap_or(self.last_seq + 1)
    }

    /// The newest line, or 0 before the first.
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }
}

/// Reads the log from the oldest line on disk.
pub(crate) struct LogReader {
    files: VecDeque<BufReader<File>>,
    buf: Vec<u8>,
}

impl LogReader {
    /// Opens both logs in `dir`. Opening them while the holder can't rotate
    /// gives a consistent pair even if it rotates while they're read.
    pub fn open(dir: &Path) -> io::Result<Self> {
        let mut files = VecDeque::new();
        for name in [EVENTS_OLD, EVENTS] {
            match File::open(dir.join(name)) {
                Ok(file) => files.push_back(BufReader::new(file)),
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
        Ok(Self {
            files,
            buf: Vec::new(),
        })
    }

    /// The next line after `after`, or `None` at the end. A partial last
    /// line ends its file, and a line that doesn't parse is skipped.
    pub fn next_after(&mut self, after: u64) -> Option<Entry> {
        while let Some(file) = self.files.front_mut() {
            self.buf.clear();
            match file.read_until(b'\n', &mut self.buf) {
                Ok(_) if self.buf.last() == Some(&b'\n') => {
                    let Ok(stored) = serde_json::from_slice::<Stored>(&self.buf) else {
                        continue;
                    };
                    if stored.seq > after {
                        return Some(Entry {
                            seq: stored.seq,
                            at: stored.at,
                            text: stored.line.into(),
                        });
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                _ => {
                    self.files.pop_front();
                }
            }
        }
        None
    }

    /// Up to `max` lines after `after`.
    pub fn batch(&mut self, after: u64, max: usize) -> Vec<Entry> {
        let mut batch = Vec::new();
        let mut after = after;
        while batch.len() < max {
            let Some(entry) = self.next_after(after) else {
                break;
            };
            after = entry.seq;
            batch.push(entry);
        }
        batch
    }
}

/// The oldest and newest line on disk in `dir`. With no lines it's `(1, 0)`.
pub(crate) fn scan(dir: &Path) -> io::Result<(u64, u64)> {
    let mut reader = LogReader::open(dir)?;
    let mut first = None;
    let mut last = 0;
    while let Some(entry) = reader.next_after(last) {
        first.get_or_insert(entry.seq);
        last = entry.seq;
    }
    Ok((first.unwrap_or(last + 1), last))
}

/// The newest lines, kept in memory up to a number of bytes.
pub(crate) struct Tail {
    entries: VecDeque<Entry>,
    bytes: usize,
    cap: usize,
}

/// What each entry costs beyond its text.
const ENTRY_COST: usize = 48;

impl Tail {
    /// An empty tail that keeps about `cap` bytes.
    pub fn new(cap: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            cap,
        }
    }

    /// Adds the newest line, dropping the oldest while over the cap. The
    /// newest line always stays.
    pub fn push(&mut self, entry: Entry) {
        self.bytes += entry.text.len() + ENTRY_COST;
        self.entries.push_back(entry);
        while self.bytes > self.cap && self.entries.len() > 1 {
            if let Some(old) = self.entries.pop_front() {
                self.bytes -= old.text.len() + ENTRY_COST;
            }
        }
    }

    /// The oldest line held.
    pub fn first_seq(&self) -> Option<u64> {
        self.entries.front().map(|entry| entry.seq)
    }

    /// Whether the line after `after` is held.
    pub fn covers(&self, after: u64) -> bool {
        self.first_seq().is_some_and(|first| first <= after + 1)
    }

    /// Up to `max` lines after `after`.
    pub fn after(&self, after: u64, max: usize) -> Vec<Entry> {
        let Some(first) = self.first_seq() else {
            return Vec::new();
        };
        let skip = usize::try_from((after + 1).saturating_sub(first)).unwrap_or(usize::MAX);
        self.entries.iter().skip(skip).take(max).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let id = maxhq_protocol::ids::SessionId::generate()
                .0
                .simple()
                .to_string();
            let path = std::env::temp_dir().join(format!("hold-{name}-{}", &id[20..]));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn text(s: &str) -> Arc<str> {
        Arc::from(s)
    }

    fn all(dir: &Path) -> Vec<u64> {
        let mut reader = LogReader::open(dir).unwrap();
        reader.batch(0, usize::MAX).iter().map(|e| e.seq).collect()
    }

    #[test]
    fn lines_are_numbered_from_one_and_read_back() {
        let dir = Dir::new("log");
        let mut log = EventLog::create(&dir.0, 1 << 20).unwrap();
        assert_eq!((log.first_seq(), log.last_seq()), (1, 0));
        for line in ["a", "b", "c"] {
            log.append(Timestamp(7), text(line));
        }
        assert_eq!((log.first_seq(), log.last_seq()), (1, 3));
        let mut reader = LogReader::open(&dir.0).unwrap();
        let read = reader.batch(1, 10);
        let texts: Vec<&str> = read.iter().map(|e| &*e.text).collect();
        assert_eq!(texts, ["b", "c"]);
        assert_eq!(scan(&dir.0).unwrap(), (1, 3));
    }

    #[test]
    fn rotation_keeps_one_old_log() {
        let dir = Dir::new("rot");
        let mut log = EventLog::create(&dir.0, 200).unwrap();
        for n in 0..40 {
            log.append(
                Timestamp(1),
                text(&format!("line {n:04} {}", "x".repeat(20))),
            );
        }
        assert!(dir.0.join(EVENTS_OLD).exists());
        let seqs = all(&dir.0);
        assert_eq!(seqs.first().copied(), Some(log.first_seq()));
        assert_eq!(seqs.last().copied(), Some(40));
        assert!(log.first_seq() > 1);
        assert!(seqs.windows(2).all(|pair| pair[1] == pair[0] + 1));
        assert_eq!(scan(&dir.0).unwrap(), (log.first_seq(), 40));
    }

    #[test]
    fn a_partial_last_line_is_skipped() {
        let dir = Dir::new("part");
        let mut log = EventLog::create(&dir.0, 1 << 20).unwrap();
        log.append(Timestamp(1), text("whole"));
        let mut file = files::append(&dir.0.join(EVENTS)).unwrap();
        file.write_all(br#"{"seq":2,"at":1,"li"#).unwrap();
        assert_eq!(all(&dir.0), [1]);
    }

    #[test]
    fn the_tail_keeps_the_newest_lines_within_its_cap() {
        let mut tail = Tail::new(3 * (ENTRY_COST + 1));
        for seq in 1..=5 {
            tail.push(Entry {
                seq,
                at: Timestamp(0),
                text: text("x"),
            });
        }
        assert_eq!(tail.first_seq(), Some(3));
        assert!(tail.covers(2));
        assert!(!tail.covers(1));
        let after: Vec<u64> = tail.after(3, 10).iter().map(|e| e.seq).collect();
        assert_eq!(after, [4, 5]);
        assert!(tail.after(5, 10).is_empty());
    }
}
