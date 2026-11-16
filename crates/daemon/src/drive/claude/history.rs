// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A session's items read back from its transcript, for a session started
//! outside MaxHQ and for one MaxHQ resumes. The items get the same IDs the
//! live stream gives them, so the two meet without duplicates.
//!
//! Each sub-agent writes a transcript of its own, `agent-<id>.jsonl` in a
//! `subagents` folder named after the session, with an `agent-<id>.meta.json`
//! beside it that names the Agent call that started it. A sub-agent's items
//! nest under that call on the lead's timeline, since a transcript read
//! back gives a sub-agent no agent of its own. A sub-agent listed as an
//! agent of its own, as a session in a terminal lists its running ones, has
//! its transcript read the same way for its own timeline.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use maxhq_protocol::ids::{AccountId, AgentId};
use maxhq_protocol::model::DEFAULT_HISTORY_MIB;
use maxhq_protocol::timeline::TimelineItem;
use serde_json::Value;
use uuid::Uuid;

use super::convo::{Convo, ToolState};
use super::tools;
use super::wire::{flag, text};
use crate::drive::pictures::Pictures;

/// How much of a transcript one read takes at most, unless the reader is
/// [`History::limit`]ed: the history an agent keeps by default. A longer
/// one is read over several calls. The first read of a longer one starts
/// this far before its end, so it shows the newest items.
const READ_CAP: u64 = (DEFAULT_HISTORY_MIB as u64) << 20;

/// The most of one unfinished line that's kept. A longer one is dropped.
const LINE_CAP: usize = 128 << 20;

/// How much a read takes from the file at once.
const CHUNK: usize = 64 << 10;

/// How much of a sub-agent's meta file is read.
const META_CAP: u64 = 1 << 20;

/// How many sub-agents deep one read follows. What's deeper waits for a
/// later read, so a long chain can't use up the stack.
const MAX_NESTING: usize = 8;

/// A transcript read a little at a time, as the session writes it.
#[derive(Debug)]
pub struct History {
    /// The agent its items belong to.
    agent: AgentId,
    /// The transcript read: the session's, or one sub-agent's.
    own: Tail,
    /// Its sub-agents' transcripts.
    subs: Subagents,
    /// The items read so far.
    convo: Convo,
}

impl History {
    /// The transcript of `session` in `config_dir`, read as `agent`'s.
    /// None when no project holds it.
    pub fn find(config_dir: &Path, session: &str, agent: AgentId) -> Option<Self> {
        Some(Self::open(transcript(config_dir, session)?, agent))
    }

    /// The transcript of sub-agent `id` of `session` in `config_dir`, read
    /// as `agent`'s, with the sub-agents it starts nested under their calls.
    /// None when no project holds the session or `id` can't be a sub-agent
    /// ID. The sub-agent's transcript may not be there yet.
    pub fn find_subagent(
        config_dir: &Path,
        session: &str,
        id: &str,
        agent: AgentId,
    ) -> Option<Self> {
        let named = !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !named {
            return None;
        }
        let dir = transcript(config_dir, session)?
            .with_extension("")
            .join("subagents");
        Some(Self {
            agent,
            own: Tail::new(dir.join(format!("agent-{id}.jsonl"))),
            subs: Subagents::new(dir, true),
            convo: quiet(agent),
        })
    }

    /// Keeps the pictures in tool results as files under `root`, the
    /// attachments root, so a read after a restart shows them again.
    pub fn keeping_pictures_in(mut self, root: PathBuf) -> Self {
        self.convo.pictures = Some(Pictures::here(root));
        self
    }

    /// The transcript at `path`, read as `agent`'s.
    pub fn open(path: PathBuf, agent: AgentId) -> Self {
        Self {
            agent,
            subs: Subagents::beside(&path),
            own: Tail::new(path),
            convo: quiet(agent),
        }
    }

    /// Names `account` in what a usage limit or a login that ran out
    /// shows, as the live session does.
    pub fn set_account(&mut self, account: AccountId) {
        self.convo.account = Some(account);
    }

    /// The transcript's path.
    pub fn path(&self) -> &Path {
        &self.own.path
    }

    /// Takes at most `bytes` from the transcript's end, and from each
    /// sub-agent's, in a read that hasn't started yet, or each whole one
    /// when there's no limit.
    pub fn limit(&mut self, bytes: Option<u64>) {
        let cap = bytes.unwrap_or(u64::MAX);
        self.own.cap = cap;
        self.subs.cap = cap;
    }

    /// The items that are new or changed since the last read, oldest
    /// first. A transcript that got shorter was rewritten, so it's read
    /// again from the start and every item comes back.
    pub fn read(&mut self) -> io::Result<Vec<TimelineItem>> {
        let (file, rewritten) = self.own.open()?;
        if rewritten {
            let pictures = self.convo.pictures.take();
            let account = self.convo.account;
            self.convo = quiet(self.agent);
            self.convo.pictures = pictures;
            self.convo.account = account;
            let cap = self.subs.cap;
            self.subs = Subagents::new(self.subs.dir.clone(), self.subs.sidechain);
            self.subs.cap = cap;
        }
        self.subs
            .follow(&mut self.own, file, &mut self.convo, |_, _| {})?;
        let items = self.convo.book.take_changed();
        self.convo.forget_settled();
        Ok(items)
    }
}

/// An empty conversation that never sends appends.
fn quiet(agent: AgentId) -> Convo {
    let mut convo = Convo::new(agent);
    convo.book.set_quiet(true);
    convo
}

/// A file of JSON lines read a little at a time, as something writes it.
/// Each line is handed on as it's read, so a read holds one line at a
/// time, never the file.
#[derive(Debug)]
struct Tail {
    /// The file.
    path: PathBuf,
    /// How far it's been read.
    offset: u64,
    /// The start of a line not yet finished.
    partial: Vec<u8>,
    /// Whether what's read up to the next line's start is dropped: the
    /// rest of a line too long to keep, or of one a read began inside.
    skipping: bool,
    /// Whether the file was opened yet.
    opened: bool,
    /// How much one read takes at most: [`READ_CAP`].
    cap: u64,
}

impl Tail {
    /// The file at `path`, not read yet.
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            partial: Vec::new(),
            skipping: false,
            opened: false,
            cap: READ_CAP,
        }
    }

    /// Opens the file where the last read stopped, and says whether it got
    /// shorter. A shorter file was rewritten, so it's read again from the
    /// start. The first read of a long file starts the cap before its end,
    /// at the next whole line.
    fn open(&mut self) -> io::Result<(File, bool)> {
        let mut file = open_regular(&self.path)?;
        let len = file.metadata()?.len();
        let rewritten = len < self.offset;
        if rewritten || !self.opened {
            self.opened = true;
            self.offset = len.saturating_sub(self.cap);
            self.partial.clear();
            self.skipping = self.offset > 0;
        }
        file.seek(SeekFrom::Start(self.offset))?;
        Ok((file, rewritten))
    }

    /// Hands `each` the record on every line finished since the last read,
    /// with the line itself, reading the `file` that [`Tail::open`] gave.
    /// A line that isn't JSON is skipped.
    fn stream(&mut self, file: File, mut each: impl FnMut(&Value, &[u8])) -> io::Result<()> {
        let mut reader = BufReader::with_capacity(CHUNK, file.take(self.cap));
        loop {
            let chunk = reader.fill_buf()?;
            if chunk.is_empty() {
                return Ok(());
            }
            let Some(end) = chunk.iter().position(|byte| *byte == b'\n') else {
                let len = chunk.len();
                if !self.skipping {
                    self.partial.extend_from_slice(chunk);
                    if self.partial.len() > LINE_CAP {
                        self.partial = Vec::new();
                        self.skipping = true;
                    }
                }
                reader.consume(len);
                self.offset += len as u64;
                continue;
            };
            if self.skipping {
                self.skipping = false;
            } else if self.partial.is_empty() {
                hand_on(&chunk[..end], &mut each);
            } else {
                self.partial.extend_from_slice(&chunk[..end]);
                hand_on(&self.partial, &mut each);
                self.partial.clear();
            }
            reader.consume(end + 1);
            self.offset += end as u64 + 1;
        }
    }
}

/// Hands `each` the record on `line`, when it holds one.
fn hand_on(line: &[u8], each: &mut impl FnMut(&Value, &[u8])) {
    if let Ok(record) = serde_json::from_slice::<Value>(line) {
        each(&record, line);
    }
}

/// Opens a file its user wrote, for a reader that may be root. A link or
/// anything else that isn't a regular file is refused, and a pipe can't
/// block the open.
fn open_regular(path: &Path) -> io::Result<File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(file)
}

/// The whole lines in the last `cap` bytes of the file at `path`.
fn read_end(path: &Path, cap: u64) -> io::Result<Vec<u8>> {
    let mut file = open_regular(path)?;
    let start = file.metadata()?.len().saturating_sub(cap);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(cap).read_to_end(&mut bytes)?;
    if start > 0 {
        let cut = bytes.iter().position(|byte| *byte == b'\n');
        bytes.drain(..cut.map_or(bytes.len(), |end| end + 1));
    }
    Ok(bytes)
}

/// The transcripts of one session's sub-agents.
#[derive(Debug)]
struct Subagents {
    /// The folder they're in.
    dir: PathBuf,
    /// Whether the transcript they're read for is a sub-agent's own, whose
    /// every record is a sidechain one.
    sidechain: bool,
    /// Each sub-agent's transcript by the Agent call that started it.
    calls: HashMap<String, Tail>,
    /// The sub-agents whose call is known, by agent ID.
    known: HashSet<String>,
    /// Whether the folder was looked through during this read.
    scanned: bool,
    /// How many sub-agents the read is inside right now.
    depth: usize,
    /// How much of each transcript one read takes at most.
    cap: u64,
}

impl Subagents {
    /// The sub-agents of the session whose transcript is at `transcript`.
    fn beside(transcript: &Path) -> Self {
        Self::new(transcript.with_extension("").join("subagents"), false)
    }

    /// The sub-agents in `dir`, read for a sub-agent's own transcript when
    /// `sidechain` is set.
    fn new(dir: PathBuf, sidechain: bool) -> Self {
        Self {
            dir,
            sidechain,
            calls: HashMap::new(),
            known: HashSet::new(),
            scanned: false,
            depth: 0,
            cap: READ_CAP,
        }
    }

    /// Reads what's new in the session's transcript `own`, from the `file`
    /// it opened, into `convo`: each sub-agent's records where its call
    /// starts and ends, then what's new from the sub-agents still running.
    /// `each` gets every record of `own` too, with its line.
    fn follow(
        &mut self,
        own: &mut Tail,
        file: File,
        convo: &mut Convo,
        mut each: impl FnMut(&Value, &[u8]),
    ) -> io::Result<()> {
        self.scanned = false;
        own.stream(file, |record, line| {
            each(record, line);
            self.apply(None, record, convo);
        })?;
        let running: Vec<String> = convo
            .book
            .items()
            .filter_map(|item| item.id.0.strip_prefix("claude:"))
            .filter(|call| {
                convo.tools.get(*call).is_some_and(|tool| {
                    tools::is_subagent(&tool.name) && tool.state != ToolState::Ended
                })
            })
            .map(str::to_owned)
            .collect();
        if running.iter().any(|call| !self.calls.contains_key(call)) {
            self.scan();
        }
        for call in running {
            self.read(&call, convo);
        }
        Ok(())
    }

    /// Reads one record of the session, or of the sub-agent that `call`
    /// started. A sub-agent's last records go in before the result of its
    /// call, and its first right after the call. A session's own sidechain
    /// records are skipped, since each sub-agent has a transcript of its own.
    fn apply(&mut self, call: Option<&str>, record: &Value, convo: &mut Convo) {
        if call.is_none() && !self.sidechain && flag(record, "isSidechain") {
            return;
        }
        for ended in blocks(record, "user", "tool_result", "tool_use_id") {
            if self.calls.contains_key(&ended) {
                self.read(&ended, convo);
            }
        }
        match call {
            None => convo.record(record),
            Some(call) => convo.sidechain(call, record),
        }
        for started in blocks(record, "assistant", "tool_use", "id") {
            let starts_agent = convo
                .tools
                .get(&started)
                .is_some_and(|tool| tools::is_subagent(&tool.name));
            if !starts_agent {
                continue;
            }
            if !self.calls.contains_key(&started) {
                self.scan();
            }
            self.read(&started, convo);
        }
    }

    /// Reads what's new in the transcript of the sub-agent that `call`
    /// started.
    fn read(&mut self, call: &str, convo: &mut Convo) {
        if self.depth >= MAX_NESTING {
            return;
        }
        // Taken out while it's read, so a transcript that names its own
        // call isn't read again inside itself.
        let Some(mut tail) = self.calls.remove(call) else {
            return;
        };
        self.depth += 1;
        if let Ok((file, _)) = tail.open() {
            // A sub-agent's transcript that can't be read now is read on a
            // later call.
            let _ = tail.stream(file, |record, _| self.apply(Some(call), record, convo));
        }
        self.depth -= 1;
        self.calls.insert(call.to_owned(), tail);
    }

    /// Looks through the folder, once a read, for sub-agents not seen
    /// before. One whose meta file doesn't name its call yet is looked at
    /// again on a later read.
    fn scan(&mut self) {
        if self.scanned {
            return;
        }
        self.scanned = true;
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(id) = name
                .to_str()
                .and_then(|name| name.strip_prefix("agent-")?.strip_suffix(".meta.json"))
            else {
                continue;
            };
            if self.known.contains(id) {
                continue;
            }
            let Some(call) = read_end(&entry.path(), META_CAP)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|meta| text(&meta, "toolUseId").map(str::to_owned))
            else {
                continue;
            };
            self.known.insert(id.to_owned());
            let transcript = self.dir.join(format!("agent-{id}.jsonl"));
            let mut tail = Tail::new(transcript);
            tail.cap = self.cap;
            self.calls.insert(call, tail);
        }
    }
}

/// The `field` of each `kind` block in the message of a `record_type`
/// record.
fn blocks(record: &Value, record_type: &str, kind: &str, field: &str) -> Vec<String> {
    if text(record, "type") != Some(record_type) {
        return Vec::new();
    }
    record
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| text(block, "type") == Some(kind))
        .filter_map(|block| text(block, field).map(str::to_owned))
        .collect()
}

/// Where the transcript of `session` is, in any project of `config_dir`.
/// The session must be a UUID, so it can't name a path elsewhere.
pub fn transcript(config_dir: &Path, session: &str) -> Option<PathBuf> {
    Uuid::parse_str(session).ok()?;
    let file = format!("{session}.jsonl");
    fs::read_dir(config_dir.join("projects"))
        .ok()?
        .flatten()
        .map(|project| project.path().join(&file))
        .find(|path| path.is_file())
}

/// The IDs of the sub-agents of `session` in `config_dir` that wrote a
/// transcript, running or not.
pub fn subagent_ids(config_dir: &Path, session: &str) -> Vec<String> {
    let Some(dir) = transcript(config_dir, session) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(dir.with_extension("").join("subagents")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let id = name
                .to_str()?
                .strip_prefix("agent-")?
                .strip_suffix(".jsonl")?;
            (!id.is_empty()).then(|| id.to_owned())
        })
        .collect()
}

/// Reads the end of the transcript of `session` into `convo`, with its
/// sub-agents' transcripts, for a driver that meets a session already
/// under way. `lead` gets each of the session's own records, with its
/// line. None when there's no transcript to read.
pub fn seed(
    config_dir: &Path,
    session: &str,
    convo: &mut Convo,
    lead: impl FnMut(&Value, &[u8]),
) -> Option<()> {
    let path = transcript(config_dir, session)?;
    let mut own = Tail::new(path.clone());
    let (file, _) = own.open().ok()?;
    Subagents::beside(&path)
        .follow(&mut own, file, convo, lead)
        .ok()
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::drive::claude::convo::{item_id, turn_id};
    use crate::testing::TempDir;
    use maxhq_protocol::timeline::{ItemBody, ItemStatus};
    use serde_json::json;

    const SESSION: &str = "fa807b40-8646-4b65-902e-8b0f6f54f532";

    fn prompt(uuid: &str, said: &str) -> String {
        format!(
            r#"{{"type":"user","uuid":"{uuid}","timestamp":"2026-10-25T09:48:42.370Z","message":{{"role":"user","content":"{said}"}}}}"#
        )
    }

    /// A session's transcript and its sub-agents' folder.
    struct Session {
        dir: TempDir,
    }

    impl Session {
        fn new() -> Self {
            let session = Self {
                dir: TempDir::new(),
            };
            fs::create_dir_all(session.subagents()).unwrap();
            session
        }

        fn transcript(&self) -> PathBuf {
            self.dir
                .path()
                .join("projects")
                .join("-home-user-work")
                .join(format!("{SESSION}.jsonl"))
        }

        fn subagents(&self) -> PathBuf {
            self.transcript().with_extension("").join("subagents")
        }

        fn history(&self) -> History {
            History::find(self.dir.path(), SESSION, AgentId::derive("lead")).unwrap()
        }

        /// Adds records to the session's transcript.
        fn lead(&self, records: &[Value]) {
            append(&self.transcript(), records);
        }

        /// Adds records to sub-agent `agent`'s transcript.
        fn sub(&self, agent: &str, records: &[Value]) {
            let records: Vec<Value> = records
                .iter()
                .cloned()
                .map(|mut record| {
                    record["isSidechain"] = json!(true);
                    record["agentId"] = json!(agent);
                    record
                })
                .collect();
            append(
                &self.subagents().join(format!("agent-{agent}.jsonl")),
                &records,
            );
        }

        /// Writes sub-agent `agent`'s meta file, naming its call.
        fn meta(&self, agent: &str, call: &str) {
            let meta = json!({
                "agentType": "general-purpose", "description": "Look around",
                "toolUseId": call, "spawnDepth": 1, "isFork": false,
            });
            fs::write(
                self.subagents().join(format!("agent-{agent}.meta.json")),
                meta.to_string(),
            )
            .unwrap();
        }
    }

    fn append(path: &Path, records: &[Value]) {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for record in records {
            writeln!(file, "{record}").unwrap();
        }
    }

    fn user(uuid: &str, content: Value) -> Value {
        json!({
            "type": "user", "uuid": uuid, "timestamp": "2026-10-25T09:48:42.000Z",
            "message": {"role": "user", "content": content},
        })
    }

    fn reply(message: &str, said: &str) -> Value {
        json!({
            "type": "assistant", "uuid": format!("a-{message}"),
            "timestamp": "2026-10-25T09:48:43.000Z",
            "message": {"id": message, "role": "assistant",
                "content": [{"type": "text", "text": said}]},
        })
    }

    fn call(message: &str, call: &str, name: &str) -> Value {
        json!({
            "type": "assistant", "uuid": format!("a-{message}"),
            "timestamp": "2026-10-25T09:48:43.000Z",
            "message": {"id": message, "role": "assistant", "content": [{
                "type": "tool_use", "id": call, "name": name,
                "input": {"description": "Look around", "prompt": "Find it", "pattern": "x"},
            }]},
        })
    }

    fn result(call: &str, said: &str) -> Value {
        user(
            &format!("r-{call}"),
            json!([{"type": "tool_result", "tool_use_id": call, "content": said}]),
        )
    }

    fn ids(items: &[TimelineItem]) -> Vec<&str> {
        items.iter().map(|item| item.id.0.as_str()).collect()
    }

    #[test]
    fn reads_new_lines_as_they_arrive_and_waits_for_a_whole_line() {
        let dir = TempDir::new();
        let project = dir.path().join("projects").join("-tmp-work");
        fs::create_dir_all(&project).unwrap();
        let path = project.join(format!("{SESSION}.jsonl"));
        fs::write(&path, format!("{}\n", prompt("u1", "one"))).unwrap();
        let mut history = History::find(dir.path(), SESSION, AgentId::derive("lead")).unwrap();
        assert_eq!(history.path(), path);
        let first = history.read().unwrap();
        assert_eq!(first.len(), 1);
        assert!(matches!(&first[0].body, ItemBody::UserMessage { text, .. } if text == "one"));
        let line = prompt("u2", "two");
        let (start, end) = line.split_at(20);
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(start.as_bytes()).unwrap();
        assert!(history.read().unwrap().is_empty());
        writeln!(file, "{end}").unwrap();
        let second = history.read().unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].id.0, "claude:u2");
        assert!(history.read().unwrap().is_empty());
    }

    #[test]
    fn a_limited_read_takes_the_newest_and_no_limit_reads_it_all() {
        let dir = TempDir::new();
        let project = dir.path().join("projects").join("-tmp-work");
        fs::create_dir_all(&project).unwrap();
        let lines: Vec<String> = (1..=9).map(|n| prompt(&format!("u{n}"), "one")).collect();
        fs::write(
            project.join(format!("{SESSION}.jsonl")),
            lines.join("\n") + "\n",
        )
        .unwrap();
        let line = lines[0].len() as u64 + 1;
        let lead = AgentId::derive("lead");

        let mut limited = History::find(dir.path(), SESSION, lead).unwrap();
        limited.limit(Some(line * 2 + 1));
        assert_eq!(ids(&limited.read().unwrap()), ["claude:u8", "claude:u9"]);

        let mut whole = History::find(dir.path(), SESSION, lead).unwrap();
        whole.limit(None);
        assert_eq!(whole.read().unwrap().len(), 9);
    }

    #[test]
    fn a_rewritten_transcript_is_read_again() {
        let dir = TempDir::new();
        let project = dir.path().join("projects").join("-tmp-work");
        fs::create_dir_all(&project).unwrap();
        let path = project.join(format!("{SESSION}.jsonl"));
        fs::write(
            &path,
            format!("{}\n{}\n", prompt("u1", "one"), prompt("u2", "two")),
        )
        .unwrap();
        let mut history = History::find(dir.path(), SESSION, AgentId::derive("lead")).unwrap();
        assert_eq!(history.read().unwrap().len(), 2);
        fs::write(&path, format!("{}\n", prompt("u3", "three"))).unwrap();
        let again = history.read().unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].id.0, "claude:u3");
    }

    #[test]
    fn only_a_uuid_names_a_transcript() {
        let dir = TempDir::new();
        assert!(transcript(dir.path(), "../../etc/passwd").is_none());
        assert!(transcript(dir.path(), SESSION).is_none());
    }

    #[test]
    fn a_sub_agents_transcript_nests_under_its_call() {
        let session = Session::new();
        session.meta("a1", "toolu_agent");
        session.sub(
            "a1",
            &[
                user("s0", json!("Find it")),
                reply("msg_sub", "Looking"),
                call("msg_sub2", "toolu_grep", "Grep"),
                result("toolu_grep", "a.txt"),
            ],
        );
        session.lead(&[
            user("u1", json!("Ask a helper")),
            call("msg_lead", "toolu_agent", "Agent"),
            result("toolu_agent", "Found it in a.txt"),
            reply("msg_end", "Done"),
        ]);
        let items = session.history().read().unwrap();
        assert_eq!(
            ids(&items),
            [
                "claude:u1",
                "claude:toolu_agent",
                "claude:msg_sub:0",
                "claude:toolu_grep",
                "claude:msg_end:0",
            ]
        );
        let lead = AgentId::derive("lead");
        for item in &items[2..4] {
            assert_eq!(item.agent, lead);
            assert_eq!(item.parent, Some(item_id("toolu_agent")), "{item:?}");
            assert_eq!(item.turn, Some(turn_id("u1")), "{item:?}");
        }
        assert_eq!(items[3].status, ItemStatus::Completed);
        assert!(matches!(
            &items[1].body,
            ItemBody::Subagent { subagent: None, .. }
        ));
    }

    #[test]
    fn a_sub_agent_is_read_as_it_writes() {
        let session = Session::new();
        session.lead(&[
            user("u1", json!("Ask a helper")),
            call("msg_lead", "toolu_agent", "Agent"),
        ]);
        let mut history = session.history();
        assert_eq!(
            ids(&history.read().unwrap()),
            ["claude:u1", "claude:toolu_agent"]
        );
        session.sub("a1", &[user("s0", json!("Find it"))]);
        assert!(history.read().unwrap().is_empty(), "no meta file yet");
        session.meta("a1", "toolu_agent");
        session.sub("a1", &[reply("msg_sub", "Looking")]);
        assert_eq!(ids(&history.read().unwrap()), ["claude:msg_sub:0"]);
        session.sub("a1", &[reply("msg_sub2", "Still looking")]);
        session.lead(&[result("toolu_agent", "Found it")]);
        assert_eq!(
            ids(&history.read().unwrap()),
            ["claude:toolu_agent", "claude:msg_sub2:0"]
        );
        session.sub("a1", &[reply("msg_late", "Late")]);
        assert!(
            history.read().unwrap().is_empty(),
            "a finished sub-agent isn't read again"
        );
    }

    #[test]
    fn a_sub_agents_own_sub_agent_nests_under_its_call() {
        let session = Session::new();
        session.meta("a1", "toolu_agent");
        session.meta("a2", "toolu_inner");
        session.sub("a2", &[reply("msg_inner", "Deep down")]);
        session.sub(
            "a1",
            &[
                call("msg_sub", "toolu_inner", "Agent"),
                result("toolu_inner", "Deep down"),
            ],
        );
        session.lead(&[
            user("u1", json!("Ask a helper")),
            call("msg_lead", "toolu_agent", "Agent"),
        ]);
        let items = session.history().read().unwrap();
        assert_eq!(
            ids(&items),
            [
                "claude:u1",
                "claude:toolu_agent",
                "claude:toolu_inner",
                "claude:msg_inner:0",
            ]
        );
        assert_eq!(items[2].parent, Some(item_id("toolu_agent")));
        assert_eq!(items[3].parent, Some(item_id("toolu_inner")));
        assert_eq!(items[3].turn, Some(turn_id("u1")));
    }

    #[test]
    fn a_sub_agent_listed_on_its_own_reads_its_own_transcript() {
        let session = Session::new();
        session.meta("a1", "toolu_agent");
        session.sub(
            "a1",
            &[
                user("s0", json!("Find it")),
                reply("msg_sub", "Looking"),
                call("msg_sub2", "toolu_grep", "Grep"),
                result("toolu_grep", "a.txt"),
            ],
        );
        session.lead(&[
            user("u1", json!("Ask a helper")),
            call("msg_lead", "toolu_agent", "Agent"),
        ]);
        let sub = AgentId::derive("sub");
        let mut history = History::find_subagent(session.dir.path(), SESSION, "a1", sub).unwrap();
        let items = history.read().unwrap();
        assert_eq!(
            ids(&items),
            ["claude:s0", "claude:msg_sub:0", "claude:toolu_grep"]
        );
        assert!(matches!(&items[0].body, ItemBody::UserMessage { text, .. } if text == "Find it"));
        for item in &items {
            assert_eq!(item.agent, sub);
            assert_eq!(item.parent, None, "{item:?}");
            assert_eq!(item.turn, Some(turn_id("s0")), "{item:?}");
        }
        assert_eq!(items[2].status, ItemStatus::Completed);

        session.sub("a1", &[reply("msg_more", "Found it")]);
        session.lead(&[reply("msg_lead2", "Waiting")]);
        assert_eq!(ids(&history.read().unwrap()), ["claude:msg_more:0"]);
    }

    #[test]
    fn a_sub_agents_own_timeline_nests_the_sub_agents_it_starts() {
        let session = Session::new();
        session.meta("a1", "toolu_agent");
        session.meta("a2", "toolu_inner");
        session.sub("a2", &[reply("msg_inner", "Deep down")]);
        session.sub(
            "a1",
            &[
                user("s0", json!("Find it")),
                call("msg_sub", "toolu_inner", "Agent"),
            ],
        );
        session.lead(&[user("u1", json!("Ask a helper"))]);
        let sub = AgentId::derive("sub");
        let items = History::find_subagent(session.dir.path(), SESSION, "a1", sub)
            .unwrap()
            .read()
            .unwrap();
        assert_eq!(
            ids(&items),
            ["claude:s0", "claude:toolu_inner", "claude:msg_inner:0"]
        );
        assert_eq!(items[2].agent, sub);
        assert_eq!(items[2].parent, Some(item_id("toolu_inner")));
    }

    #[test]
    fn only_a_plain_id_names_a_sub_agents_transcript() {
        let session = Session::new();
        let sub = AgentId::derive("sub");
        assert!(
            History::find_subagent(session.dir.path(), SESSION, "a1", sub).is_none(),
            "no session transcript"
        );
        session.lead(&[user("u1", json!("Ask a helper"))]);
        for id in ["", "../a1", "a1/../../x", "."] {
            assert!(History::find_subagent(session.dir.path(), SESSION, id, sub).is_none());
        }
        let mut history = History::find_subagent(session.dir.path(), SESSION, "a1", sub).unwrap();
        assert!(history.read().is_err(), "no transcript yet");
        session.sub("a1", &[user("s0", json!("Find it"))]);
        assert_eq!(ids(&history.read().unwrap()), ["claude:s0"]);
    }

    #[test]
    fn a_seed_reads_the_sub_agents_too() {
        let session = Session::new();
        session.meta("a1", "toolu_agent");
        session.sub("a1", &[reply("msg_sub", "Looking")]);
        session.lead(&[
            user("u1", json!("Ask a helper")),
            call("msg_lead", "toolu_agent", "Agent"),
        ]);
        let mut convo = quiet(AgentId::derive("lead"));
        let mut lead = 0;
        seed(session.dir.path(), SESSION, &mut convo, |_, _| lead += 1).expect("a transcript");
        assert_eq!(lead, 2);
        assert!(convo.book.contains(&item_id("msg_sub:0")));
    }

    /// Runs `read` on a thread of its own and fails if it doesn't come back.
    fn returns<T: Send + 'static>(read: impl FnOnce() -> T + Send + 'static) -> T {
        let (done, back) = std::sync::mpsc::channel();
        std::thread::spawn(move || done.send(read()));
        back.recv_timeout(std::time::Duration::from_secs(10))
            .expect("the read blocked")
    }

    fn pipe(path: &Path) {
        let made = std::process::Command::new("mkfifo").arg(path).status();
        assert!(made.unwrap().success());
    }

    #[test]
    fn a_pipe_planted_as_a_transcript_or_meta_file_doesnt_block_a_read() {
        let session = Session::new();
        session.meta("a1", "toolu_agent");
        pipe(&session.subagents().join("agent-a1.jsonl"));
        pipe(&session.subagents().join("agent-a2.meta.json"));
        session.lead(&[call("msg_lead", "toolu_agent", "Agent")]);
        let mut history = session.history();
        let items = returns(move || history.read().unwrap());
        assert!(ids(&items).contains(&item_id("toolu_agent").0.as_str()));
        pipe(&session.dir.path().join("own.jsonl"));
        let mut own = History::open(session.dir.path().join("own.jsonl"), AgentId::derive("own"));
        assert!(returns(move || own.read()).is_err());
    }

    #[test]
    fn a_link_planted_as_a_transcript_or_meta_file_isnt_followed() {
        let session = Session::new();
        let elsewhere = session.dir.path().join("elsewhere");
        append(&elsewhere, &[reply("msg_elsewhere", "Not the agent's")]);
        let meta = session.dir.path().join("meta");
        fs::write(&meta, json!({"toolUseId": "toolu_meta"}).to_string()).unwrap();
        std::os::unix::fs::symlink(&meta, session.subagents().join("agent-a2.meta.json")).unwrap();
        session.meta("a1", "toolu_agent");
        std::os::unix::fs::symlink(&elsewhere, session.subagents().join("agent-a1.jsonl")).unwrap();
        session.lead(&[call("msg_lead", "toolu_agent", "Agent")]);
        let mut history = session.history();
        let items = history.read().unwrap();
        assert!(!ids(&items).contains(&item_id("msg_elsewhere:0").0.as_str()));
        assert!(!history.subs.calls.contains_key("toolu_meta"));
    }

    #[test]
    fn a_long_chain_of_sub_agents_is_read_a_few_levels_at_a_time() {
        let session = Session::new();
        let last = MAX_NESTING * 8;
        session.lead(&[call("m0", "c0", "Agent")]);
        for level in 0..last {
            let agent = format!("s{level}");
            session.meta(&agent, &format!("c{level}"));
            let next = level + 1;
            session.sub(
                &agent,
                &[call(&format!("m{next}"), &format!("c{next}"), "Agent")],
            );
        }
        let mut history = session.history();
        history.read().unwrap();
        let deepest = format!("c{last}");
        assert!(history.convo.tools.contains_key("c1"));
        assert!(!history.convo.tools.contains_key(&deepest));
        for _ in 0..last {
            history.read().unwrap();
        }
        assert!(history.convo.tools.contains_key(&deepest));
    }

    #[test]
    fn the_end_of_a_long_file_starts_at_a_whole_line() {
        let dir = TempDir::new();
        let path = dir.path().join("long");
        fs::write(&path, "one\ntwo\nthree\n").unwrap();
        assert_eq!(read_end(&path, 100).unwrap(), b"one\ntwo\nthree\n");
        assert_eq!(read_end(&path, 8).unwrap(), b"three\n");
        assert_eq!(read_end(&path, 3).unwrap(), b"");
    }

    /// The records `tail` reads next.
    fn records(tail: &mut Tail) -> Vec<Value> {
        let (file, _) = tail.open().unwrap();
        let mut records = Vec::new();
        tail.stream(file, |record, _| records.push(record.clone()))
            .unwrap();
        records
    }

    #[test]
    fn a_line_past_the_cap_is_dropped_and_reading_goes_on() {
        let dir = TempDir::new();
        let path = dir.path().join("transcript.jsonl");
        fs::write(&path, "").unwrap();
        let mut tail = Tail::new(path.clone());
        assert!(records(&mut tail).is_empty());
        tail.partial = vec![b'x'; LINE_CAP];
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(file, "no end of line").unwrap();
        assert!(records(&mut tail).is_empty());
        assert!(tail.partial.is_empty());
        writeln!(file, "{}", json!({"rest": true})).unwrap();
        assert!(
            records(&mut tail).is_empty(),
            "the dropped line's end isn't a record"
        );
        append(&path, &[json!({"next": true})]);
        assert_eq!(records(&mut tail), vec![json!({"next": true})]);
    }

    #[test]
    fn the_first_read_of_a_long_file_starts_near_its_end_at_a_whole_line() {
        let dir = TempDir::new();
        let path = dir.path().join("transcript.jsonl");
        let old = json!({"n": 1}).to_string();
        let new = json!({"n": 2}).to_string();
        fs::write(&path, format!("{old}\n{old}\n{new}\n{new}\n")).unwrap();
        let mut tail = Tail::new(path.clone());
        tail.cap = new.len() as u64 * 2 + 3;
        assert_eq!(records(&mut tail), vec![json!({"n": 2}), json!({"n": 2})]);
        append(&path, &[json!({"n": 3})]);
        assert_eq!(records(&mut tail), vec![json!({"n": 3})]);
    }

    #[test]
    fn lines_split_across_reads_and_chunks_come_whole() {
        let dir = TempDir::new();
        let path = dir.path().join("transcript.jsonl");
        let long = "x".repeat(CHUNK * 2 + 7);
        let line = json!({"long": long}).to_string();
        let (start, end) = line.split_at(CHUNK + 3);
        fs::write(&path, start).unwrap();
        let mut tail = Tail::new(path.clone());
        assert!(records(&mut tail).is_empty());
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{end}").unwrap();
        writeln!(file, "not json").unwrap();
        writeln!(file, "{}", json!({"short": 1})).unwrap();
        assert_eq!(
            records(&mut tail),
            vec![json!({"long": long}), json!({"short": 1})]
        );
    }

    #[test]
    fn lists_the_sub_agents_that_wrote_a_transcript() {
        let session = Session::new();
        assert!(subagent_ids(session.dir.path(), SESSION).is_empty());
        session.lead(&[json!({"type": "user"})]);
        session.sub("a1", &[json!({"type": "user"})]);
        session.meta("a2", "toolu_2");
        fs::write(session.subagents().join("notes.txt"), "").unwrap();
        assert_eq!(subagent_ids(session.dir.path(), SESSION), ["a1"]);
        assert!(subagent_ids(session.dir.path(), "not-a-session").is_empty());
    }
}
