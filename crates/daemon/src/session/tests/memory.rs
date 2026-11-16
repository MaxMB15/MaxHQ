// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A benchmark of `maxd` coming back over long Claude transcripts: its
//! memory footprint at its peak and once settled, how long until it
//! answers, and how long until every agent shows the end of its history.
//! Each run goes in a process of its own, so no other test's memory counts.
//! [`ITEMS_VAR`] sets how many items each agent keeps in memory.

use std::env;
use std::fs::{self, File};
use std::io::{BufWriter, Write as _};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Instant;

use maxhq_protocol::model::DeviceSettingChange;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::*;

/// The variable that tells a run what to measure.
const SPEC_VAR: &str = "MAXD_MEMORY_BENCH";

/// The variable that sets the recent items each agent keeps in memory,
/// for a benchmark at other than the default.
const ITEMS_VAR: &str = "MAXD_MEMORY_ITEMS";

/// What a run's report line starts with.
const MARK: &str = "memory bench: ";

/// How long a run may take before it fails, at any size.
const LONG: Duration = Duration::from_secs(600);

/// The newest item of each transcript.
const LAST: &str = "claude:msg_last:0";

const MIB: u64 = 1 << 20;

/// How many agents a run has, and how many bytes of transcript each has,
/// its sub-agents' included.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct Size {
    agents: usize,
    bytes: u64,
}

/// What one run measures and where its transcripts are.
#[derive(Serialize, Deserialize)]
struct Spec {
    size: Size,
    source: PathBuf,
    /// The recent items each agent keeps in memory, or the default.
    #[serde(default)]
    items: Option<u16>,
}

/// What a run measured. Footprints are in MiB, and none where this system
/// has no measure.
#[derive(Debug, Serialize, Deserialize)]
struct Measured {
    /// When the run started, before any agent.
    base: Option<f64>,
    /// The most the process ever held.
    peak: Option<f64>,
    /// A second after every agent was ready.
    settled: Option<f64>,
    /// From the restart until the manager answered a client.
    answer_ms: u128,
    /// From the restart until every agent showed its newest item.
    ready_ms: u128,
}

/// The most a run may take.
struct Limits {
    peak: f64,
    settled: f64,
    answer: Duration,
    ready: Duration,
}

#[test]
fn a_restart_over_long_transcripts_stays_small_and_quick() {
    let size = Size {
        agents: 3,
        bytes: 2 * MIB,
    };
    bench(
        size,
        // Well above a debug build on a busy machine, which peaks near
        // 13 MiB and is ready in under a second, so only a real
        // regression trips it.
        &Limits {
            peak: 128.0,
            settled: 128.0,
            answer: Duration::from_secs(30),
            ready: Duration::from_secs(60),
        },
    );
}

#[test]
#[ignore = "writes about 500 MB of transcripts and takes minutes"]
fn a_restart_over_twelve_agents_and_500_mb_stays_small_and_quick() {
    let size = Size {
        agents: 12,
        bytes: 42 * MIB,
    };
    bench(
        size,
        &Limits {
            peak: 4096.0,
            settled: 4096.0,
            answer: LONG,
            ready: LONG,
        },
    );
}

#[tokio::test]
async fn a_memory_setting_out_of_range_is_kept_to_the_nearest() {
    let mut h = Harness::new();
    let device = device().id;
    let mut save = async |changes: Vec<DeviceSettingChange>| {
        let outcome = h
            .act(|key| ClientMessage::UpdateDeviceSettings {
                key,
                device,
                changes,
            })
            .await;
        assert_eq!(outcome, ActionOutcome::Done);
        let kept = h.db.device_settings(device).unwrap().unwrap();
        (kept.memory_items, kept.history_mib)
    };
    let kept = save(vec![
        DeviceSettingChange::MemoryItems { value: 9999 },
        DeviceSettingChange::HistoryMib { value: 1 },
    ])
    .await;
    assert_eq!(kept, (4096, 32));
    let kept = save(vec![
        DeviceSettingChange::MemoryItems { value: 1 },
        DeviceSettingChange::HistoryMib { value: 0 },
    ])
    .await;
    assert_eq!(kept, (64, 0), "no limit stays no limit");
}

/// Writes the transcripts, measures a run over them in a process of its
/// own, and checks it stayed within `limits`.
fn bench(size: Size, limits: &Limits) {
    let source = TempDir::new();
    for index in 0..size.agents {
        write_transcript(source.path(), &format!("t{index}"), size.bytes);
    }
    let spec = Spec {
        size,
        source: source.path().to_owned(),
        items: env::var(ITEMS_VAR)
            .ok()
            .and_then(|items| items.parse().ok()),
    };
    let out = Command::new(env::current_exe().expect("the test knows its program"))
        .args([
            "--exact",
            "session::tests::memory::one_run",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(
            SPEC_VAR,
            serde_json::to_string(&spec).expect("a spec serializes"),
        )
        .output()
        .expect("the run starts");
    let said = String::from_utf8_lossy(&out.stdout);
    let errors = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the run ended with {}:\n{said}\n{errors}",
        out.status
    );
    let measured: Measured = said
        .lines()
        .find_map(|line| line.split_once(MARK))
        .map(|(_, line)| serde_json::from_str(line).expect("the report parses"))
        .unwrap_or_else(|| panic!("the run reported nothing:\n{said}\n{errors}"));
    eprintln!("{size:?}: {measured:?}");
    if let Some(peak) = measured.peak {
        assert!(peak <= limits.peak, "the peak was {peak:.0} MiB");
    }
    if let Some(settled) = measured.settled {
        assert!(settled <= limits.settled, "{settled:.0} MiB stayed");
    }
    assert!(
        measured.answer_ms <= limits.answer.as_millis(),
        "the first answer took {} ms",
        measured.answer_ms
    );
    assert!(
        measured.ready_ms <= limits.ready.as_millis(),
        "the agents were ready after {} ms",
        measured.ready_ms
    );
}

/// One run, which [`bench`] starts with the spec in [`SPEC_VAR`]. It starts
/// the agents, gives each a transcript, restarts, and has each CLI print
/// its first line, which reads the transcript.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a run of the memory benchmark, which starts it in a process of its own"]
async fn one_run() {
    let Ok(spec) = env::var(SPEC_VAR) else {
        return;
    };
    let spec: Spec = serde_json::from_str(&spec).expect("the spec parses");
    let base = footprint().map(|(now, _)| now);
    let mut h = Harness::new();
    h.vendors.use_real_claude();
    let mut agents = Vec::new();
    for _ in 0..spec.size.agents {
        let (agent, session) = h.start_claude(None).await;
        let (uuid, _) = *h.vendors.launches().last().expect("the agent launched");
        agents.push((agent, session, uuid));
    }
    let project = h
        .dir
        .path()
        .join("claude")
        .join("projects")
        .join("-home-user-project");
    fs::create_dir_all(&project).expect("the project folder can be made");
    for (index, (_, _, uuid)) in agents.iter().enumerate() {
        let from = spec.source.join(format!("t{index}"));
        fs::rename(
            from.with_extension("jsonl"),
            project.join(format!("{uuid}.jsonl")),
        )
        .expect("the transcript moves in");
        fs::rename(&from, project.join(uuid.to_string())).expect("the sub-agents move in");
    }
    if let Some(value) = spec.items {
        let outcome = h
            .act(|key| ClientMessage::UpdateDeviceSettings {
                key,
                device: device().id,
                changes: vec![DeviceSettingChange::MemoryItems { value }],
            })
            .await;
        assert_eq!(outcome, ActionOutcome::Done);
    }

    h.restart().await;
    let booted = Instant::now();
    let mut client = h.sessions.client(ClientId::derive("bench"));
    let mut seqs = Vec::new();
    let mut answer = None;
    for (agent, _, _) in &agents {
        let seq = match open(&mut client, *agent, None, 1).await {
            TimelineUpdate::Reset { seq, .. } => seq,
            other => panic!("expected a reset, got {other:?}"),
        };
        answer.get_or_insert_with(|| booted.elapsed());
        seqs.push(seq);
    }
    for (_, session, uuid) in &agents {
        let init = json!({
            "type": "system", "subtype": "init", "session_id": uuid.to_string(),
            "cwd": "/home/user/project", "model": "claude-opus-4-5",
            "permissionMode": "default", "tools": [],
        });
        h.holders.print(*session, init.to_string());
    }
    // An agent is ready once its history came in again, and its timeline
    // ends as its transcript does.
    let mut waiting: Vec<(AgentId, Seq)> = agents
        .iter()
        .map(|(agent, _, _)| *agent)
        .zip(seqs)
        .collect();
    while !waiting.is_empty() {
        assert!(
            booted.elapsed() < LONG,
            "{} agents never got ready",
            waiting.len()
        );
        let mut still = Vec::new();
        for (agent, seq) in waiting {
            match open(&mut client, agent, Some(seq), 8).await {
                TimelineUpdate::Reset { items, .. }
                    if items.iter().any(|item| item.id.0 == LAST) => {}
                _ => still.push((agent, seq)),
            }
        }
        waiting = still;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let ready = booted.elapsed();
    tokio::time::sleep(Duration::from_secs(1)).await;
    let now = footprint();
    let measured = Measured {
        base: base.map(mib),
        peak: now.map(|(_, peak)| mib(peak)),
        settled: now.map(|(now, _)| mib(now)),
        answer_ms: answer.unwrap_or(ready).as_millis(),
        ready_ms: ready.as_millis(),
    };
    println!(
        "{MARK}{}",
        serde_json::to_string(&measured).expect("a report serializes")
    );
}

/// Opens an agent's timeline and waits for what comes back, dropping
/// everything else.
async fn open(
    client: &mut Client,
    agent: AgentId,
    since: Option<Seq>,
    limit: u32,
) -> TimelineUpdate {
    client.send(ClientMessage::OpenTimeline {
        agent,
        since,
        limit,
        lead: None,
    });
    loop {
        let message = tokio::time::timeout(LONG, client.recv())
            .await
            .expect("the timeline came")
            .expect("the manager runs");
        if let ServerMessage::Timeline { agent: got, update } = message
            && got == agent
        {
            return update;
        }
    }
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / MIB as f64
}

/// The process's footprint now and at its most, in bytes: the physical
/// footprint on macOS, as Activity Monitor shows it.
#[cfg(target_os = "macos")]
fn footprint() -> Option<(u64, u64)> {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    #[allow(unsafe_code)]
    // SAFETY: the buffer is a zeroed `rusage_info_v4`, the size the flavor
    // asks for, and it outlives the call.
    let code = unsafe {
        libc::proc_pid_rusage(
            std::process::id() as libc::c_int,
            libc::RUSAGE_INFO_V4,
            info.as_mut_ptr().cast(),
        )
    };
    if code != 0 {
        return None;
    }
    #[allow(unsafe_code)]
    // SAFETY: the call succeeded, so it filled the buffer in, and every
    // field is a plain number anyway.
    let info = unsafe { info.assume_init() };
    Some((info.ri_phys_footprint, info.ri_lifetime_max_phys_footprint))
}

/// The process's resident memory now and at its most, in bytes.
#[cfg(target_os = "linux")]
fn footprint() -> Option<(u64, u64)> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let field = |name: &str| {
        status.lines().find_map(|line| {
            let kib = line.strip_prefix(name)?.trim().strip_suffix("kB")?;
            kib.trim().parse::<u64>().ok().map(|kib| kib * 1024)
        })
    };
    Some((field("VmRSS:")?, field("VmHWM:")?))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn footprint() -> Option<(u64, u64)> {
    None
}

/// Writes a transcript like a long working session's at `<dir>/<name>.jsonl`,
/// with its sub-agents' under `<dir>/<name>/subagents`, until they hold
/// about `bytes`. It ends with the message [`LAST`] names.
fn write_transcript(dir: &Path, name: &str, bytes: u64) {
    let subagents = dir.join(name).join("subagents");
    fs::create_dir_all(&subagents).expect("the sub-agents folder can be made");
    let mut lead = Writer::new(&dir.join(format!("{name}.jsonl")), None);
    let mut turn = 0;
    let mut sub_bytes = 0;
    while lead.written + sub_bytes < bytes {
        sub_bytes += write_turn(&mut lead, &subagents, turn);
        turn += 1;
    }
    lead.prompt(&format!("u{turn}"), &filler(turn, 200));
    lead.assistant("msg_last", json!({"type": "text", "text": "All done."}));
    lead.turn_ends(&format!("d{turn}"));
    lead.out.flush().expect("the transcript is written");
}

/// Writes one turn: a prompt, thinking, a few tools with results the size
/// real ones have, sometimes a sub-agent, and a reply. Returns how many
/// bytes the sub-agent's transcript took.
fn write_turn(lead: &mut Writer, subagents: &Path, turn: usize) -> u64 {
    let t = turn * 16;
    lead.prompt(&format!("u{turn}"), &filler(t, 300));
    let message = format!("msg_{turn}a");
    lead.assistant(
        &message,
        json!({"type": "thinking", "thinking": filler(t + 1, 1500), "signature": filler(t + 2, 400)}),
    );
    lead.assistant(
        &message,
        json!({"type": "text", "text": filler(t + 3, 600)}),
    );

    let call = format!("toolu_{turn}_bash");
    lead.assistant(
        &format!("msg_{turn}b"),
        json!({"type": "tool_use", "id": call, "name": "Bash",
            "input": {"command": "cargo test -p app", "description": "Run the tests"}}),
    );
    let output = filler(t + 4, 6000);
    lead.result(
        &call,
        json!(output),
        json!({"stdout": output, "stderr": "", "interrupted": false, "isImage": false}),
    );

    let call = format!("toolu_{turn}_read");
    lead.assistant(
        &format!("msg_{turn}c"),
        json!({"type": "tool_use", "id": call, "name": "Read",
            "input": {"file_path": "/home/user/project/src/main.rs"}}),
    );
    let file = filler(t + 5, 12000);
    lead.result(
        &call,
        json!(file),
        json!({"type": "text", "file": {"filePath": "/home/user/project/src/main.rs",
            "content": file, "numLines": 300, "startLine": 1, "totalLines": 300}}),
    );

    let call = format!("toolu_{turn}_edit");
    let (old, new) = (filler(t + 6, 800), filler(t + 7, 1000));
    lead.assistant(
        &format!("msg_{turn}d"),
        json!({"type": "tool_use", "id": call, "name": "Edit",
            "input": {"file_path": "/home/user/project/src/main.rs",
                "old_string": old, "new_string": new}}),
    );
    lead.result(
        &call,
        json!(format!(
            "The file /home/user/project/src/main.rs has been updated. Here's the result:\n{}",
            filler(t + 8, 1000)
        )),
        json!({"filePath": "/home/user/project/src/main.rs", "oldString": old,
            "newString": new, "originalFile": filler(t + 9, 20000),
            "structuredPatch": [{"oldStart": 10, "oldLines": 5, "newStart": 10,
                "newLines": 6, "lines": [filler(t + 10, 2000)]}],
            "userModified": false, "replaceAll": false}),
    );

    let sub_bytes = if turn % 20 == 7 {
        write_subagent(lead, subagents, turn)
    } else {
        0
    };
    lead.assistant(
        &format!("msg_{turn}e"),
        json!({"type": "text", "text": filler(t + 11, 800)}),
    );
    lead.turn_ends(&format!("d{turn}"));
    sub_bytes
}

/// An Agent call, its sub-agent's own transcript and meta file, and its
/// result. Returns how many bytes the sub-agent's transcript took.
fn write_subagent(lead: &mut Writer, subagents: &Path, turn: usize) -> u64 {
    let t = turn * 16;
    let id = format!("a{turn}");
    let call = format!("toolu_{turn}_agent");
    let prompt = filler(t + 12, 500);
    let meta = json!({"agentType": "general-purpose", "description": "Look around",
        "toolUseId": call, "spawnDepth": 1, "isFork": false});
    fs::write(
        subagents.join(format!("agent-{id}.meta.json")),
        meta.to_string(),
    )
    .expect("the meta file is written");
    lead.assistant(
        &format!("msg_{turn}f"),
        json!({"type": "tool_use", "id": call, "name": "Agent",
            "input": {"description": "Look around", "prompt": prompt,
                "subagent_type": "general-purpose"}}),
    );

    let mut sub = Writer::new(&subagents.join(format!("agent-{id}.jsonl")), Some(&id));
    sub.prompt(&format!("{id}u"), &prompt);
    let message = format!("msg_{id}a");
    sub.assistant(
        &message,
        json!({"type": "thinking", "thinking": filler(t + 13, 800), "signature": "sig"}),
    );
    let grep = format!("toolu_{id}_grep");
    sub.assistant(
        &format!("msg_{id}b"),
        json!({"type": "tool_use", "id": grep, "name": "Grep", "input": {"pattern": "fn main"}}),
    );
    let found = filler(t + 14, 4000);
    sub.result(
        &grep,
        json!(found),
        json!({"mode": "content", "content": found, "numLines": 80}),
    );
    let read = format!("toolu_{id}_read");
    sub.assistant(
        &format!("msg_{id}c"),
        json!({"type": "tool_use", "id": read, "name": "Read",
            "input": {"file_path": "/home/user/project/src/lib.rs"}}),
    );
    let file = filler(t + 15, 12000);
    sub.result(
        &read,
        json!(file),
        json!({"type": "text", "file": {"filePath": "/home/user/project/src/lib.rs",
            "content": file, "numLines": 300, "startLine": 1, "totalLines": 300}}),
    );
    let said = filler(t + 16, 1000);
    sub.assistant(&format!("msg_{id}d"), json!({"type": "text", "text": said}));
    sub.out
        .flush()
        .expect("the sub-agent's transcript is written");

    lead.result(
        &call,
        json!([{"type": "text", "text": said}]),
        json!({"status": "completed", "prompt": prompt, "agentId": id,
            "content": [{"type": "text", "text": said}], "totalDurationMs": 64000,
            "totalTokens": 30000, "totalToolUseCount": 2}),
    );
    sub.written
}

/// Writes transcript records, each linked to the one before.
struct Writer {
    out: BufWriter<File>,
    written: u64,
    parent: Option<String>,
    /// The sub-agent the transcript is for, if it's one's.
    agent: Option<String>,
}

impl Writer {
    fn new(path: &Path, agent: Option<&str>) -> Self {
        Self {
            out: BufWriter::new(File::create(path).expect("the transcript can be made")),
            written: 0,
            parent: None,
            agent: agent.map(str::to_owned),
        }
    }

    fn record(&mut self, kind: &str, uuid: &str, fields: Value) {
        let mut record = json!({
            "parentUuid": self.parent, "isSidechain": self.agent.is_some(),
            "userType": "external", "cwd": "/home/user/project",
            "sessionId": "fa807b40-8646-4b65-902e-8b0f6f54f532", "version": "2.0.31",
            "gitBranch": "main", "type": kind, "uuid": uuid,
            "timestamp": "2026-10-25T09:48:42.000Z",
        });
        if let Some(agent) = &self.agent {
            record["agentId"] = json!(agent);
        }
        if let (Value::Object(record), Value::Object(fields)) = (&mut record, fields) {
            record.extend(fields);
        }
        let line = serde_json::to_vec(&record).expect("a record serializes");
        self.out.write_all(&line).expect("the record is written");
        self.out.write_all(b"\n").expect("the record is written");
        self.written += line.len() as u64 + 1;
        self.parent = Some(uuid.to_owned());
    }

    fn prompt(&mut self, uuid: &str, said: &str) {
        self.record(
            "user",
            uuid,
            json!({"message": {"role": "user", "content": said}}),
        );
    }

    fn assistant(&mut self, message: &str, block: Value) {
        let uuid = format!("{message}-{}", self.written);
        self.record(
            "assistant",
            &uuid,
            json!({"requestId": format!("req_{message}"), "message": {
                "id": message, "type": "message", "role": "assistant",
                "model": "claude-opus-4-5", "content": [block],
                "stop_reason": null, "stop_sequence": null,
                "usage": {"input_tokens": 4, "cache_creation_input_tokens": 1200,
                    "cache_read_input_tokens": 90000, "output_tokens": 350,
                    "service_tier": "standard"}}}),
        );
    }

    fn result(&mut self, call: &str, content: Value, result: Value) {
        self.record(
            "user",
            &format!("r-{call}"),
            json!({"message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": call, "content": content}]},
                "toolUseResult": result}),
        );
    }

    fn turn_ends(&mut self, uuid: &str) {
        self.record(
            "system",
            uuid,
            json!({"subtype": "turn_duration", "durationMs": 45000}),
        );
    }
}

/// `len` bytes of text that read like prose, picked by `seed`.
fn filler(seed: usize, len: usize) -> String {
    static TEXT: OnceLock<String> = OnceLock::new();
    let text = TEXT.get_or_init(|| {
        const WORDS: [&str; 16] = [
            "the", "agent", "reads", "a", "file", "and", "runs", "tests", "while", "maxd", "keeps",
            "its", "timeline", "in", "order,", "then\n",
        ];
        let mut state: u32 = 0x9e37_79b9;
        let mut text = String::with_capacity(70_000);
        while text.len() < 64 * 1024 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            text.push_str(WORDS[(state >> 28) as usize]);
            text.push(' ');
        }
        text
    });
    let len = len.min(text.len());
    let start = seed.wrapping_mul(7919) % (text.len() - len + 1);
    text[start..start + len].to_owned()
}
