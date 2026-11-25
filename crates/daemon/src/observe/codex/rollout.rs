// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Reads a Codex rollout, the JSON lines file a session appends to. The
//! first line describes the session, and the `event_msg` lines after it say
//! when each turn starts and ends. Anything unknown is skipped, since every
//! Codex release adds events.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use maxhq_core::context;
use maxhq_protocol::model::{Activity, ContextUsage};
use maxhq_protocol::time::Timestamp;
use serde::Deserialize;
use serde_json::Value;

use super::clock::{self, LocalClock};
use crate::usage::codex::limits::{self, Reading};

/// How long the vendor is assumed to keep a conversation cached after a
/// turn, per decision C-7. Codex doesn't say, so this is an estimate.
pub const CACHE_WARM: Duration = Duration::from_secs(5 * 60);

/// How full a window is when its limit stops new turns.
const USED_UP: f64 = 100.0;

/// The longest title taken from a prompt.
const TITLE_CHARS: usize = 60;

/// The thread ID in a rollout's file name, which is
/// `rollout-<local time>-<thread ID>.jsonl`, sometimes with a suffix after
/// an underscore.
pub fn thread_id(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let rest = name.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
    // The time is `YYYY-MM-DDTHH-MM-SS`, 19 characters, then a dash.
    if rest.as_bytes().get(19) != Some(&b'-') {
        return None;
    }
    let id = rest.get(20..)?.split('_').next()?;
    uuid::Uuid::parse_str(id).ok()?;
    Some(id.to_owned())
}

/// Where a read of a file stopped, so the next one picks up there.
#[derive(Debug, Default)]
pub struct Tail {
    offset: u64,
    /// The start of a line whose end isn't written yet.
    partial: Vec<u8>,
    /// Which file it is, to notice it being replaced.
    identity: Option<maxhq_os::fsmode::FileId>,
}

/// What a tail read found.
#[derive(Debug, PartialEq, Eq)]
pub enum Tailed<'a> {
    /// The file was replaced or cut short, so everything read before is
    /// void and the lines after this start from its beginning.
    Restart,
    /// One whole line, without its line break.
    Line(&'a str),
}

impl Tail {
    /// Reads the lines written since the last read and passes each to
    /// `each`. A line that isn't UTF-8 is skipped.
    pub fn read(&mut self, path: &Path, mut each: impl FnMut(Tailed<'_>)) -> io::Result<()> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        let identity = maxhq_os::fsmode::file_id(&file).ok();
        if identity != self.identity || len < self.offset {
            if self.offset > 0 || !self.partial.is_empty() {
                each(Tailed::Restart);
            }
            self.offset = 0;
            self.partial.clear();
            self.identity = identity;
        }
        if len == self.offset {
            return Ok(());
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut reader =
            BufReader::with_capacity(64 * 1024, io::Read::take(file, len - self.offset));
        let mut line = std::mem::take(&mut self.partial);
        loop {
            let read = reader.read_until(b'\n', &mut line)?;
            if read == 0 {
                break;
            }
            self.offset += read as u64;
            if line.last() == Some(&b'\n') {
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                if let Ok(text) = std::str::from_utf8(&line) {
                    each(Tailed::Line(text));
                }
                line.clear();
            }
        }
        self.partial = line;
        Ok(())
    }
}

/// Who started a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// You did, in a terminal, an editor or `codex exec`.
    Lead,
    /// Another thread spawned it as a sub-agent.
    Subagent {
        /// The thread that spawned it.
        parent: String,
        /// The name Codex gave it, such as "Ada".
        nickname: Option<String>,
        /// Its role, such as "explorer".
        role: Option<String>,
    },
    /// Codex runs it for itself, such as a review or a compaction, and it
    /// isn't shown.
    Helper,
}

impl Source {
    /// Reads `session_meta.source`.
    fn parse(source: Option<&Value>) -> Self {
        let Some(subagent) = source.and_then(|source| source.get("subagent")) else {
            return Self::Lead;
        };
        let Some(spawn) = subagent.get("thread_spawn") else {
            return Self::Helper;
        };
        let text = |key: &str| {
            spawn
                .get(key)
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        match text("parent_thread_id") {
            Some(parent) => Self::Subagent {
                parent,
                nickname: text("agent_nickname"),
                role: text("agent_role"),
            },
            None => Self::Helper,
        }
    }
}

/// What a rollout's `session_meta` line says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    /// The folder the session works in.
    pub cwd: Option<PathBuf>,
    /// When the session started.
    pub started_at: Option<Timestamp>,
    /// Who started it.
    pub source: Source,
}

impl Meta {
    /// Reads a `session_meta` payload written at `written`.
    fn parse(payload: &Value, written: Option<Timestamp>) -> Self {
        // Older releases nest the fields one level down.
        let meta = payload
            .get("meta")
            .filter(|meta| meta.is_object())
            .unwrap_or(payload);
        Self {
            cwd: meta
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|cwd| !cwd.is_empty())
                .map(PathBuf::from),
            started_at: meta
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(clock::parse_utc)
                .or(written),
            source: Source::parse(meta.get("source")),
        }
    }
}

/// Where the latest turn stands.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Turn {
    /// No turn has started.
    #[default]
    None,
    /// A turn is running, or waiting on an approval, which the rollout
    /// doesn't show.
    Running,
    /// The turn finished, at the time given when the rollout says.
    Done(Option<Timestamp>),
    /// The turn hit a usage limit that resets at the time given.
    LimitHit(Option<Timestamp>),
    /// The turn failed or you interrupted it.
    Stopped,
}

/// What a rollout says so far about its session.
#[derive(Clone, Debug, Default)]
pub struct Rollout {
    /// The session's description, once its first line is read.
    pub meta: Option<Meta>,
    turn: Turn,
    /// When the session last did something: a turn starting or ending, or
    /// any line written during a turn.
    pub active_at: Option<Timestamp>,
    /// How full the context window was at the last token count.
    pub context: Option<ContextUsage>,
    /// The rate limits at the last token count.
    pub rate_limits: Option<Reading>,
    /// A title made from the first prompt.
    pub first_prompt: Option<String>,
    /// The model's window from the last turn start, for token counts that
    /// leave it out.
    window: Option<u32>,
}

/// One line of a rollout, with the payload left as JSON.
#[derive(Deserialize)]
struct Line {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    payload: Value,
}

/// The `event_msg` types that change what the observer shows.
const EVENTS: [&str; 6] = [
    "\"task_started\"",
    "\"turn_started\"",
    "\"task_complete\"",
    "\"turn_complete\"",
    "\"turn_aborted\"",
    "\"token_count\"",
];

impl Rollout {
    /// Reads every line of a rollout from the start.
    #[cfg(test)]
    pub fn from_text(text: &str, clock: &impl LocalClock) -> Self {
        let mut rollout = Self::default();
        for line in text.lines() {
            rollout.read_line(line, clock);
        }
        rollout
    }

    /// Takes in one line. A line that isn't JSON, or that describes
    /// something the observer doesn't show, changes nothing beyond the time
    /// of the last activity.
    pub fn read_line(&mut self, text: &str, clock: &impl LocalClock) {
        // Most lines are model output. Checking for the few names that
        // matter is far cheaper than parsing every one.
        if !self.wanted(text) {
            if self.turn == Turn::Running {
                self.touch(quick_time(text));
            }
            return;
        }
        let Ok(line) = serde_json::from_str::<Line>(text) else {
            return;
        };
        let at = line.timestamp.as_deref().and_then(clock::parse_utc);
        if self.turn == Turn::Running {
            self.touch(at);
        }
        match line.kind.as_str() {
            "session_meta" if self.meta.is_none() => {
                self.meta = Some(Meta::parse(&line.payload, at));
            }
            "event_msg" => self.event(&line.payload, at, clock),
            _ => {}
        }
    }

    /// What the session is doing at `now`.
    pub fn activity(&self, now: Timestamp) -> Activity {
        match self.turn {
            Turn::None | Turn::Stopped | Turn::Done(None) => Activity::Idle,
            Turn::Running => Activity::Busy,
            Turn::Done(Some(at)) => {
                let warm_until = at.after(CACHE_WARM);
                if now < warm_until {
                    Activity::IdleCacheWarm { warm_until }
                } else {
                    Activity::Idle
                }
            }
            Turn::LimitHit(resets_at) => match resets_at.or_else(|| self.used_up_until()) {
                // Codex waits for a new prompt once the limit resets.
                Some(at) if at <= now => Activity::Idle,
                resets_at => Activity::WaitingForLimitReset { resets_at },
            },
        }
    }

    /// Whether a line could change anything, judged without parsing it.
    fn wanted(&self, text: &str) -> bool {
        if self.meta.is_none() && text.contains("\"session_meta\"") {
            return true;
        }
        if !text.contains("\"event_msg\"") {
            return false;
        }
        EVENTS.iter().any(|event| text.contains(event))
            || (self.first_prompt.is_none()
                && (text.contains("\"user_message\"") || text.contains("\"UserMessage\"")))
    }

    /// Reads one `event_msg` payload.
    fn event(&mut self, payload: &Value, at: Option<Timestamp>, clock: &impl LocalClock) {
        let Some(kind) = payload.get("type").and_then(Value::as_str) else {
            return;
        };
        match kind {
            "task_started" | "turn_started" => {
                self.turn = Turn::Running;
                if let Some(window) = payload.get("model_context_window").and_then(as_u32) {
                    self.window = Some(window);
                }
                self.touch(at);
            }
            "task_complete" | "turn_complete" => {
                self.turn = finished(payload, at, clock);
                self.touch(at);
            }
            "turn_aborted" => {
                self.turn = Turn::Stopped;
                self.touch(at);
            }
            "token_count" => self.token_count(payload, at),
            "user_message" => {
                if let Some(message) = payload.get("message").and_then(Value::as_str) {
                    self.prompt(message);
                }
            }
            "item_completed" => {
                let item = &payload["item"];
                if item.get("type").and_then(Value::as_str) == Some("UserMessage") {
                    let text: Vec<&str> = item["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect();
                    self.prompt(&text.join("\n"));
                }
            }
            _ => {}
        }
    }

    /// Reads the context and rate limits in a `token_count` event.
    fn token_count(&mut self, payload: &Value, at: Option<Timestamp>) {
        let info = &payload["info"];
        let used = info["last_token_usage"]["input_tokens"].as_u64();
        let window = info
            .get("model_context_window")
            .and_then(as_u32)
            .or(self.window);
        if let (Some(used), Some(window)) = (used, window)
            && window > 0
        {
            self.context = Some(ContextUsage {
                used_tokens: u32::try_from(used).unwrap_or(u32::MAX),
                window_tokens: window,
            });
        }
        if let Some(at) = at
            && let Some(reading) = payload
                .get("rate_limits")
                .and_then(|rate_limits| limits::from_rollout(rate_limits, at))
        {
            self.rate_limits = Some(reading);
        }
    }

    /// Keeps the first prompt as the title, its pills shown as the app
    /// shows them and without their context block.
    fn prompt(&mut self, text: &str) {
        if self.first_prompt.is_none() {
            self.first_prompt = title(&context::readable(text));
        }
    }

    /// Moves the last activity time forward.
    fn touch(&mut self, at: Option<Timestamp>) {
        if let Some(at) = at {
            self.active_at = Some(self.active_at.map_or(at, |active| active.max(at)));
        }
    }

    /// When the latest limit that's used up resets, for a limit message
    /// without a time.
    fn used_up_until(&self) -> Option<Timestamp> {
        self.rate_limits
            .iter()
            .flat_map(|reading| &reading.limits)
            .flat_map(|limit| limit.windows.iter().flatten())
            .filter(|window| window.used_percent >= USED_UP)
            .filter_map(|window| window.resets_at)
            .max()
    }
}

/// How a `task_complete` event ended its turn.
fn finished(payload: &Value, at: Option<Timestamp>, clock: &impl LocalClock) -> Turn {
    let Some(error) = payload.get("error").filter(|error| !error.is_null()) else {
        let completed = payload
            .get("completed_at")
            .and_then(Value::as_u64)
            .map(|secs| Timestamp(secs.saturating_mul(1000)));
        return Turn::Done(at.or(completed));
    };
    if error_kind(error) != Some("usage_limit_exceeded") {
        return Turn::Stopped;
    }
    let message = error.get("message").and_then(Value::as_str).unwrap_or("");
    Turn::LimitHit(at.and_then(|at| clock::limit_resets_at(message, at, clock)))
}

/// The kind of an error's `codex_error_info`, which is a plain string for
/// most kinds and an object with one key for kinds that carry details.
fn error_kind(error: &Value) -> Option<&str> {
    match error.get("codex_error_info")? {
        Value::String(kind) => Some(kind),
        Value::Object(fields) if fields.len() == 1 => fields.keys().next().map(String::as_str),
        _ => None,
    }
}

/// A title from the first line of a prompt.
fn title(prompt: &str) -> Option<String> {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let mut title: String = line.chars().take(TITLE_CHARS).collect();
    if line.chars().nth(TITLE_CHARS).is_some() {
        title.truncate(title.trim_end().len());
        title.push('…');
    }
    Some(title)
}

/// The time at the start of a rollout line, without parsing the rest.
fn quick_time(text: &str) -> Option<Timestamp> {
    let rest = text.strip_prefix("{\"timestamp\":\"")?;
    clock::parse_utc(rest.split_once('"')?.0)
}

/// A JSON number that fits in a `u32`.
fn as_u32(value: &Value) -> Option<u32> {
    u32::try_from(value.as_u64()?).ok()
}

#[cfg(test)]
mod tests {
    use super::super::clock::tests::FixedClock;
    use super::*;
    use std::io::Write;

    const LEAD: &str = include_str!("fixtures/lead.jsonl");
    const SUBAGENT: &str = include_str!("fixtures/subagent.jsonl");
    const LIMIT: &str = include_str!("fixtures/limit.jsonl");
    const ERRORS: &str = include_str!("fixtures/errors.jsonl");

    /// New York in October, four hours behind UTC.
    const CLOCK: FixedClock = FixedClock(-4 * 60);

    fn at(text: &str) -> Timestamp {
        clock::parse_utc(text).unwrap()
    }

    /// The rollout after each of its lines in turn.
    fn states(text: &str) -> Vec<Rollout> {
        let mut rollout = Rollout::default();
        text.lines()
            .map(|line| {
                rollout.read_line(line, &CLOCK);
                rollout.clone()
            })
            .collect()
    }

    #[test]
    fn thread_ids_come_from_file_names() {
        let id = "0199f0a1-7c1e-7a52-9d1e-3f4a5b6c7d8e";
        let cases = [
            (format!("rollout-2026-10-24T09-12-03-{id}.jsonl"), Some(id)),
            (
                format!("rollout-2026-10-24T09-12-03-{id}_2.jsonl"),
                Some(id),
            ),
            (format!("rollout-2026-10-24T09-12-03-{id}.jsonl.zst"), None),
            (
                "rollout-2026-10-24T09-12-03-not-a-uuid.jsonl".to_owned(),
                None,
            ),
            (format!("rollout-{id}.jsonl"), None),
            (format!("{id}.jsonl"), None),
        ];
        for (name, want) in cases {
            let path = Path::new("/x/sessions/2026/10/24").join(&name);
            assert_eq!(thread_id(&path).as_deref(), want, "{name}");
        }
    }

    #[test]
    fn a_lead_session_goes_busy_then_idle_and_warm() {
        let rollout = Rollout::from_text(LEAD, &CLOCK);
        let meta = rollout.meta.as_ref().unwrap();
        assert_eq!(meta.source, Source::Lead);
        assert_eq!(
            meta.cwd.as_deref(),
            Some(Path::new("/Users/dev/src/my app"))
        );
        assert_eq!(meta.started_at, Some(at("2026-10-24T13:12:03.100Z")));
        assert_eq!(
            rollout.first_prompt.as_deref(),
            Some("Add a dark mode toggle to the settings page, and keep the li…")
        );
        assert_eq!(
            rollout.context,
            Some(ContextUsage {
                used_tokens: 18_234,
                window_tokens: 258_400,
            })
        );
        let done = at("2026-10-24T13:12:31.250Z");
        assert_eq!(
            rollout.active_at,
            Some(done),
            "settings after the turn aren't activity"
        );
        let warm_until = done.after(CACHE_WARM);
        assert_eq!(
            rollout.activity(done.after(Duration::from_secs(60))),
            Activity::IdleCacheWarm { warm_until }
        );
        assert_eq!(rollout.activity(warm_until), Activity::Idle);

        let states = states(LEAD);
        let now = done;
        let activity: Vec<Activity> = states.iter().map(|state| state.activity(now)).collect();
        assert_eq!(
            activity[0],
            Activity::Idle,
            "a new session waits for a prompt"
        );
        assert!(
            activity[1..6].iter().all(|a| *a == Activity::Busy),
            "{activity:?}"
        );
        assert_eq!(activity[6], Activity::IdleCacheWarm { warm_until });
        assert_eq!(
            states[3].active_at,
            Some(at("2026-10-24T13:12:07.400Z")),
            "model output during a turn is activity"
        );
    }

    #[test]
    fn rollout_rate_limits_are_read_at_the_line_time() {
        let rollout = Rollout::from_text(LEAD, &CLOCK);
        let reading = rollout.rate_limits.unwrap();
        assert_eq!(reading.checked_at, at("2026-10-24T13:12:09.500Z"));
        assert_eq!(
            reading.source,
            maxhq_protocol::model::UsageSource::CodexRollout
        );
        let windows = reading.limits[0].windows.as_ref().unwrap();
        let shape: Vec<_> = windows
            .iter()
            .map(|w| (w.scope.as_deref(), w.window_minutes, w.used_percent))
            .collect();
        assert_eq!(shape, vec![(None, 300, 41.0), (None, 10_080, 12.0)]);
    }

    #[test]
    fn a_usage_limit_waits_for_the_reset_time() {
        let rollout = Rollout::from_text(LIMIT, &CLOCK);
        // The message says 3:05 PM, written at 10:00 in UTC-4.
        let resets_at = at("2026-10-27T19:05:00Z");
        let after = at("2026-10-27T14:00:01Z");
        assert_eq!(
            rollout.activity(after),
            Activity::WaitingForLimitReset {
                resets_at: Some(resets_at),
            }
        );
        assert_eq!(rollout.activity(resets_at), Activity::Idle);
    }

    #[test]
    fn a_usage_limit_without_a_time_uses_the_full_window() {
        let text = LIMIT.replace(" or try again at 3:05 PM.", ". Try again later.");
        let rollout = Rollout::from_text(&text, &CLOCK);
        assert_eq!(
            rollout.activity(at("2026-10-27T14:00:01Z")),
            Activity::WaitingForLimitReset {
                resets_at: Some(at("2026-10-27T19:05:12Z")),
            }
        );
    }

    #[test]
    fn errors_and_interrupts_leave_the_session_idle() {
        let now = at("2026-10-25T08:03:21Z");
        let activity: Vec<Activity> = states(ERRORS).iter().map(|s| s.activity(now)).collect();
        assert_eq!(
            activity,
            vec![
                Activity::Idle,
                Activity::Busy,
                // An error Codex calls "other".
                Activity::Idle,
                Activity::Busy,
                // An error with details.
                Activity::Idle,
                Activity::Busy,
                // You pressed Esc.
                Activity::Idle,
                // The newer event names, and an explicit null error.
                Activity::Busy,
                Activity::IdleCacheWarm {
                    warm_until: at("2026-10-25T08:03:20Z").after(CACHE_WARM),
                },
            ]
        );
        assert_eq!(
            states(ERRORS)[0].meta.as_ref().map(|m| &m.source),
            Some(&Source::Lead),
            "codex exec counts as yours"
        );
    }

    #[test]
    fn a_subagent_names_its_parent() {
        let rollout = Rollout::from_text(SUBAGENT, &CLOCK);
        assert_eq!(
            rollout.meta.as_ref().unwrap().source,
            Source::Subagent {
                parent: "0199f0a1-7c1e-7a52-9d1e-3f4a5b6c7d8e".into(),
                nickname: Some("Ada".into()),
                role: Some("explorer".into()),
            }
        );
        assert_eq!(
            rollout.first_prompt.as_deref(),
            Some("Find where the theme is stored")
        );
        assert_eq!(rollout.activity(Timestamp(0)), Activity::Busy);
        assert_eq!(
            rollout.context,
            Some(ContextUsage {
                used_tokens: 9_100,
                window_tokens: 128_000,
            })
        );
    }

    #[test]
    fn helper_sessions_are_marked() {
        for source in [
            serde_json::json!({ "subagent": "review" }),
            serde_json::json!({ "subagent": { "other": "memory" } }),
            serde_json::json!({ "subagent": { "thread_spawn": { "depth": 1 } } }),
        ] {
            assert_eq!(Source::parse(Some(&source)), Source::Helper, "{source}");
        }
        assert_eq!(Source::parse(Some(&"vscode".into())), Source::Lead);
        assert_eq!(Source::parse(None), Source::Lead);
    }

    #[test]
    fn bad_and_unknown_lines_are_skipped() {
        let mut rollout = Rollout::default();
        for line in [
            "",
            "not json",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"",
            "{\"timestamp\":\"2026-10-24T13:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"brand_new_event\",\"task_started\":1}}",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":null,\"rate_limits\":null}}",
            "[1,2,3]",
        ] {
            rollout.read_line(line, &CLOCK);
        }
        assert_eq!(rollout.activity(Timestamp(0)), Activity::Idle);
        assert_eq!(rollout.context, None);
        assert_eq!(rollout.rate_limits, None);
        assert_eq!(rollout.meta, None);
    }

    #[test]
    fn older_meta_nests_its_fields() {
        let line = r#"{"timestamp":"2026-01-02T03:04:05.000Z","type":"session_meta","payload":{"meta":{"id":"x","cwd":"/srv/app","source":"cli"},"git":null}}"#;
        let rollout = Rollout::from_text(line, &CLOCK);
        let meta = rollout.meta.unwrap();
        assert_eq!(meta.cwd.as_deref(), Some(Path::new("/srv/app")));
        assert_eq!(meta.started_at, Some(at("2026-01-02T03:04:05Z")));
    }

    #[test]
    fn titles_take_the_first_line() {
        assert_eq!(
            title("\n  Fix the build  \nthen test"),
            Some("Fix the build".into())
        );
        assert_eq!(title(" \n "), None);
        let long = "word ".repeat(30);
        let cut = title(&long).unwrap();
        assert!(cut.ends_with("word…"), "{cut}");
        assert!(cut.chars().count() <= TITLE_CHARS + 1);
    }

    #[test]
    fn a_first_prompt_with_pills_shows_them_without_their_block() {
        use maxhq_core::attached::Attached;
        use maxhq_protocol::context::{InlineRef, QuoteSource};
        use maxhq_protocol::ids::{AgentId, AttachmentId};
        use maxhq_protocol::timeline::{AttachmentKind, ItemId};

        let lead: AgentId = "0199a7c2-0000-7000-8000-000000000001".parse().unwrap();
        let shot = context::File {
            id: AttachmentId::derive("/path/to/shot.png"),
            attached: Attached {
                kind: AttachmentKind::Image,
                name: "shot.png".into(),
                path: "/path/to/shot.png".into(),
                text: None,
                holds: None,
            },
            sent: context::Sent::Inline,
        };
        let refs = [
            InlineRef::Attachment { id: shot.id },
            InlineRef::Quote {
                agent: lead,
                agent_title: "api".into(),
                item: ItemId("codex:item_01".into()),
                text: "the cache never expires".into(),
                position: 0,
                source: QuoteSource::Reply,
                from: None,
            },
        ];
        let sent = context::encode("fix \u{FFFC} like \u{FFFC}", &refs, &[shot], lead);
        assert!(sent.text.contains("<maxhq_context>"));
        let line = serde_json::json!({
            "timestamp": "2026-10-24T13:30:41.210Z",
            "type": "event_msg",
            "payload": { "type": "user_message", "message": sent.text, "images": [] },
        });
        let mut rollout = Rollout::default();
        rollout.read_line(&line.to_string(), &CLOCK);
        assert_eq!(
            rollout.first_prompt.as_deref(),
            Some("fix shot.png like \"the cache never expires\"")
        );
    }

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("maxd-codex-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn collect(tail: &mut Tail, path: &Path) -> Vec<String> {
        let mut seen = Vec::new();
        tail.read(path, |read| {
            seen.push(match read {
                Tailed::Restart => "<restart>".to_owned(),
                Tailed::Line(text) => text.to_owned(),
            });
        })
        .unwrap();
        seen
    }

    #[test]
    fn a_tail_reads_only_whole_new_lines() {
        let path = temp_file("append.jsonl");
        std::fs::write(&path, "one\ntwo\r\nthr").unwrap();
        let mut tail = Tail::default();
        assert_eq!(collect(&mut tail, &path), ["one", "two"]);
        assert!(collect(&mut tail, &path).is_empty(), "nothing new");

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"ee\nfour\n").unwrap();
        assert_eq!(collect(&mut tail, &path), ["three", "four"]);

        // Cut short, as when a file is rewritten in place.
        std::fs::write(&path, "new\n").unwrap();
        assert_eq!(collect(&mut tail, &path), ["<restart>", "new"]);

        // Replaced by another file under the same name.
        let other = temp_file("replacement.jsonl");
        std::fs::write(&other, "new\nlonger than before\n").unwrap();
        std::fs::rename(&other, &path).unwrap();
        assert_eq!(
            collect(&mut tail, &path),
            ["<restart>", "new", "longer than before"]
        );

        std::fs::remove_file(&path).unwrap();
        assert!(
            tail.read(&path, |_| {}).is_err(),
            "a missing file is an error"
        );
        let _ = std::fs::remove_dir(path.parent().unwrap());
    }
}
