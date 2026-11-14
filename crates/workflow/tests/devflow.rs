// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! devflow as the first user of the standard. Each fixture under
//! `fixtures/devflow/runs/<case>/<variant>/` holds a devflow run folder at
//! several steps, made by devflow's own `df-state.sh` with a fixed clock:
//! `patched` with the proposed devflow patch, `plain` without it, `hand`
//! written by hand in an older devflow's shape. The test mounts the
//! descriptor, the run and its config in a project in memory, derives each
//! step with one [`Run`] and checks the result against `step-N.json` beside
//! it. Set `MAXHQ_REGENERATE` to rewrite those files.
//!
//! Markdown files are kept as `<name>.md.txt` so the repo holds no Markdown,
//! and mounted under their real names.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use maxhq_protocol::ids::{AgentId, DeviceId, ProjectId};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::workflow::{NodeState, NodeStatus, RunSnapshot};
use maxhq_workflow::fake::{FakeAgents, MemFiles};
use maxhq_workflow::{AgentState, Catalog, Run, Workflow};

const PROJECT: &str = "/work/example-app";
const VENDOR: &str = "/work/example-app/.devflow-vendor";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/devflow")
}

/// Milliseconds since the epoch for an RFC 3339 time.
fn ms(time: &str) -> u64 {
    let t: jiff::Timestamp = time.parse().expect("an RFC 3339 time");
    u64::try_from(t.as_millisecond()).expect("after 1970")
}

/// Every file under `dir`, relative to it, with `.md.txt` names mounted as
/// `.md`.
fn tree(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_owned()];
    while let Some(at) = todo.pop() {
        for entry in std::fs::read_dir(&at).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                todo.push(path);
                continue;
            }
            let rel = path
                .strip_prefix(dir)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let rel = rel
                .strip_suffix(".txt")
                .filter(|r| r.ends_with(".md"))
                .unwrap_or(&rel);
            out.push((PathBuf::from(rel), std::fs::read(&path).unwrap()));
        }
    }
    out.sort();
    out
}

/// A project holding the vendored devflow descriptor and its block
/// manifests, as `.devflow-vendor/` would.
fn project() -> MemFiles {
    let mut files = MemFiles::default();
    let root = fixtures();
    files.put(
        Path::new(VENDOR).join(".maxhq/workflows/devflow.yaml"),
        std::fs::read(root.join("devflow.yaml")).unwrap(),
    );
    for (rel, bytes) in tree(&root.join("vendor")) {
        files.put(Path::new(VENDOR).join(rel), bytes);
    }
    files
}

/// The agents around one step of a case.
struct Around {
    now: &'static str,
    agents: FakeAgents,
}

impl Around {
    fn new(now: &'static str) -> Self {
        let mut agents = FakeAgents::default();
        agents.now = Timestamp(ms(now));
        Self { now, agents }
    }

    /// The orchestrator, always `fake/0`, so a patched run's `task.agent`
    /// names it.
    fn lead(mut self, title: &str, state: AgentState, command: Option<&str>) -> Self {
        let id = self.agents.add(title, None);
        self.agents.state(id, state);
        self.agents.started(id, ms("2026-11-01T00:00:00Z"));
        self.agents.wrote(id, ms(self.now), false);
        if let Some(command) = command {
            self.agents.command(id, command);
        }
        self
    }

    /// A sub-agent under the orchestrator.
    fn sub(mut self, title: &str, state: AgentState, wrote: &str) -> Self {
        let id = self.agents.add(title, Some(lead()));
        self.agents.state(id, state);
        self.agents.started(id, ms(wrote));
        self.agents.wrote(id, ms(wrote), false);
        self
    }

    /// An agent started on its own with a dispatch prompt, as a Codex
    /// sub-agent on another host is.
    fn prompted(mut self, prompt: &str, wrote: &str) -> Self {
        let id = self.agents.add("Codex", None);
        self.agents.prompt(id, prompt);
        self.agents.started(id, ms(wrote));
        self.agents.wrote(id, ms(wrote), false);
        self
    }
}

fn lead() -> AgentId {
    AgentId::derive("fake/0")
}

/// One fixture: a run folder at successive steps.
struct Case {
    case: &'static str,
    variant: &'static str,
    task: &'static str,
    steps: Vec<Around>,
}

impl Case {
    fn dir(&self) -> PathBuf {
        fixtures().join("runs").join(self.case).join(self.variant)
    }

    fn run_dir(&self) -> PathBuf {
        Path::new(PROJECT).join(".devflow/state").join(self.task)
    }

    /// Writes step `n`'s run folder into the project, removing what it no
    /// longer holds.
    fn mount(&self, files: &mut MemFiles, n: usize, held: &mut BTreeSet<PathBuf>) {
        let now: BTreeSet<PathBuf> = tree(&self.dir().join(format!("step-{n}")))
            .into_iter()
            .map(|(rel, bytes)| {
                let path = self.run_dir().join(rel);
                files.put(&path, bytes);
                path
            })
            .collect();
        for gone in held.difference(&now) {
            files.remove(gone);
        }
        *held = now;
    }

    fn derive_all(self) -> Vec<RunSnapshot> {
        let mut files = project();
        let config = self.dir().join("config.yaml");
        if config.exists() {
            files.put(
                Path::new(PROJECT).join(".devflow/config.yaml"),
                std::fs::read(config).unwrap(),
            );
        }
        let mut held = BTreeSet::new();
        self.mount(&mut files, 1, &mut held);

        let scan = Catalog::default().scan(&files, Path::new(PROJECT));
        assert!(scan.problems.is_empty(), "{:?}", scan.problems);
        let workflow: &Workflow = scan.workflow("devflow").expect("the vendored descriptor");
        let runs = scan.runs(&files);
        let found = runs
            .iter()
            .find(|r| r.workflow == "devflow")
            .expect("the run");
        assert_eq!(found.file, self.run_dir().join("task.yaml"));
        assert_eq!(found.root, Path::new(VENDOR));

        let mut run = Run::new(
            DeviceId::derive("devbox"),
            ProjectId::derive("example-app"),
            found.clone(),
        );
        run.set_retry_pause(Duration::ZERO);
        let mut out = Vec::new();
        for (i, around) in self.steps.iter().enumerate() {
            let n = i + 1;
            if n > 1 {
                self.mount(&mut files, n, &mut held);
            }
            let snapshot = run.derive(&workflow.descriptor, &files, &around.agents);
            let golden = serde_json::json!({
                "snapshot": snapshot,
                "summary": run.summary(),
                "header_lines": run.header_lines(),
                "orchestrators": run.orchestrators(),
            });
            self.check(n, &golden);
            out.push(snapshot);
        }
        out
    }

    /// Compares one step with its golden file, or rewrites it.
    fn check(&self, n: usize, golden: &serde_json::Value) {
        let mut text = serde_json::to_string_pretty(golden).unwrap();
        for i in 0..8 {
            let id = serde_json::to_string(&AgentId::derive(&format!("fake/{i}"))).unwrap();
            text = text.replace(&id, &format!("\"agent fake/{i}\""));
        }
        text.push('\n');
        let path = self.dir().join(format!("step-{n}.json"));
        if std::env::var_os("MAXHQ_REGENERATE").is_some() {
            std::fs::write(&path, text).unwrap();
            return;
        }
        let want = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!("{} is missing. run with MAXHQ_REGENERATE=1", path.display())
        });
        assert!(
            want == text,
            "{} differs. run with MAXHQ_REGENERATE=1 and check the diff\n{text}",
            path.display()
        );
    }
}

fn node<'a>(s: &'a RunSnapshot, id: &str) -> &'a NodeState {
    s.nodes
        .iter()
        .find(|n| n.id == id)
        .unwrap_or_else(|| panic!("no node {id}"))
}

fn status(s: &RunSnapshot, id: &str) -> NodeStatus {
    node(s, id).status
}

fn badges(s: &RunSnapshot, id: &str) -> Vec<String> {
    node(s, id).badges.iter().map(|b| b.text.clone()).collect()
}

const FEATURE: &str = "t-20261101-example-feature";

/// The same run, with or without the patch. A plain run has no
/// `task.agent`, so its lead binds through the command it ran.
fn feature(variant: &'static str) -> Case {
    let init = (variant == "plain").then_some(
        "bash .devflow-vendor/scripts/df-state.sh init t-20261101-example-feature \
         --request 'Add an export button to the report view' --type non-ui",
    );
    let lead = |now, state| Around::new(now).lead("Add an export button", state, init);
    let title = |block: &str| format!("{block} {FEATURE}");
    Case {
        case: "feature",
        variant,
        task: FEATURE,
        steps: vec![
            // The interview asks you.
            lead("2026-11-01T09:03:00Z", AgentState::Waiting),
            // Research runs; chaos and perf are off under small_feature.
            lead("2026-11-01T09:06:00Z", AgentState::Busy).sub(
                &title("df-research"),
                AgentState::Busy,
                "2026-11-01T09:05:30Z",
            ),
            // Design runs on a Codex agent that has gone quiet.
            lead("2026-11-01T09:25:00Z", AgentState::Busy).prompted(
                "You run one devflow block.\nblock: df-design\ntask: t-20261101-example-feature\n",
                "2026-11-01T09:10:00Z",
            ),
            // Chunk 2 of 3 of the plan.
            lead("2026-11-01T09:19:30Z", AgentState::Busy).sub(
                &title("df-implement"),
                AgentState::Busy,
                "2026-11-01T09:19:10Z",
            ),
            // The analysis gates, one of them already ended.
            lead("2026-11-01T09:31:00Z", AgentState::Busy)
                .sub(
                    &title("df-testing"),
                    AgentState::Busy,
                    "2026-11-01T09:30:30Z",
                )
                .sub(
                    &title("df-security"),
                    AgentState::Gone,
                    "2026-11-01T09:30:40Z",
                )
                .sub(
                    &title("df-edge-cases"),
                    AgentState::Busy,
                    "2026-11-01T09:30:50Z",
                ),
            // Done, after a validate and integrate loop.
            lead("2026-11-01T09:58:00Z", AgentState::Idle),
        ],
    }
}

#[test]
fn a_patched_run_reads_step_by_step() {
    let s = feature("patched").derive_all();
    assert_eq!(status(&s[0], "df-interview"), NodeStatus::WaitingHuman);
    assert_eq!(status(&s[1], "df-chaos"), NodeStatus::WillSkip);
    assert!(node(&s[1], "df-chaos").provisional);
    assert_eq!(status(&s[2], "df-perf"), NodeStatus::WillSkip);
    assert!(!node(&s[2], "df-perf").provisional);
    assert!(node(&s[2], "df-design").quiet);
    assert!(badges(&s[3], "df-implement").contains(&"2/3".to_owned()));
    assert_eq!(
        node(&s[4], "df-chaos").note.as_deref(),
        Some("effort off under small_feature")
    );
    assert!(badges(&s[4], "df-code-quality").contains(&"1 open".to_owned()));
    assert!(badges(&s[5], "df-source-control").contains(&"owner/repo#42".to_owned()));
    assert!(s[5].header.status == NodeStatus::Done);
}

#[test]
fn a_plain_run_reads_without_the_patch() {
    let plain = feature("plain").derive_all();
    let patched = feature("patched").derive_all();
    // Without `effort` nothing is known ahead: chaos waits like any node.
    assert_eq!(status(&plain[1], "df-chaos"), NodeStatus::Queued);
    assert!(badges(&plain[1], "df-research")
        .iter()
        .all(|b| b != "light"));
    // Everything the journal and task file say reads the same either way.
    for (a, b) in plain.iter().zip(&patched) {
        for (x, y) in a.nodes.iter().zip(&b.nodes) {
            if x.status != NodeStatus::Queued && x.status != NodeStatus::Ready {
                assert_eq!((&x.id, x.status), (&y.id, y.status));
            }
        }
        assert_eq!(a.loops, b.loops);
        assert_eq!(a.shape, b.shape);
    }
}

#[test]
fn a_reclassify_replans_what_will_be_skipped() {
    let task = "t-20261101-example-spike";
    let s = Case {
        case: "reclassify",
        variant: "patched",
        task,
        steps: vec![
            Around::new("2026-11-01T11:06:00Z")
                .lead("Try a faster report query", AgentState::Busy, None)
                .sub(
                    &format!("df-research {task}"),
                    AgentState::Busy,
                    "2026-11-01T11:05:30Z",
                ),
            Around::new("2026-11-01T11:10:00Z").lead(
                "Try a faster report query",
                AgentState::Busy,
                None,
            ),
        ],
    }
    .derive_all();
    for id in ["df-testing", "df-edge-cases", "df-chaos", "df-perf"] {
        assert_eq!(status(&s[0], id), NodeStatus::WillSkip, "{id}");
        assert_eq!(status(&s[1], id), NodeStatus::Queued, "{id}");
    }
    assert!(badges(&s[1], "df-edge-cases").contains(&"thorough".to_owned()));
}

fn ceiling() -> Case {
    Case {
        case: "ceiling",
        variant: "patched",
        task: "t-20261101-example-ceiling",
        steps: vec![
            // The orchestrator is still on it, so the failure is only a
            // failure; one retry is left.
            Around::new("2026-11-01T13:37:00Z").lead("Fix the date filter", AgentState::Busy, None),
            // devflow refused the rerun, so the orchestrator asks you.
            Around::new("2026-11-01T13:42:00Z").lead(
                "Fix the date filter",
                AgentState::Waiting,
                None,
            ),
        ],
    }
}

#[test]
fn a_spent_ceiling_blocks_the_rerun_devflow_refused() {
    let s = ceiling().derive_all();
    assert_eq!(status(&s[0], "df-integrate"), NodeStatus::Failed);
    let s = &s[1..];
    let validate = node(&s[0], "df-validate");
    assert_eq!(validate.status, NodeStatus::Blocked);
    assert_eq!(validate.note.as_deref(), Some("retry ceiling spent"));
    // A first dispatch is never refused, so the nodes after it only wait.
    assert_eq!(status(&s[0], "df-docs"), NodeStatus::Queued);
    assert!(badges(&s[0], "df-integrate").contains(&"x2".to_owned()));
}

#[test]
fn a_failed_block_the_orchestrator_retries_leaves_the_run_going() {
    let s = ceiling().derive_all();
    // df-integrate failed once, the lead is busy and five nodes still wait.
    assert_ne!(s[0].header.status, NodeStatus::Failed);
}

#[test]
fn a_mockup_run_waits_on_its_children_then_fails_together() {
    let task = "t-20261101-example-mockup";
    let init = Some(
        "bash .devflow-vendor/scripts/df-state.sh init t-20261101-example-mockup \
         --request 'Redesign the settings page' --type ui",
    );
    let lead = |now, state| Around::new(now).lead("Redesign the settings page", state, init);
    let s = Case {
        case: "mockup",
        variant: "plain",
        task,
        steps: vec![
            lead("2026-11-01T15:12:30Z", AgentState::Busy).sub(
                &format!("df-mockup {task}"),
                AgentState::Busy,
                "2026-11-01T15:11:00Z",
            ),
            lead("2026-11-01T15:15:00Z", AgentState::Waiting),
            lead("2026-11-01T15:16:00Z", AgentState::Idle),
            Around::new("2026-11-01T15:20:00Z").lead(
                "Redesign the settings page",
                AgentState::Gone,
                init,
            ),
        ],
    }
    .derive_all();
    assert_eq!(status(&s[0], "df-approve"), NodeStatus::WaitingChild);
    assert_eq!(node(&s[0], "df-approve").links.len(), 1);
    assert_eq!(
        node(&s[0], "df-approve").links[0].url,
        "http://127.0.0.1:5173/"
    );
    assert_eq!(status(&s[1], "df-approve"), NodeStatus::WaitingHuman);
    assert!(badges(&s[1], "df-approve").contains(&"round 2".to_owned()));
    for id in ["df-approve", "df-mockup"] {
        assert_eq!(status(&s[2], id), NodeStatus::WaitingHuman, "{id}");
        assert_eq!(node(&s[2], id).note.as_deref(), Some("failed, run stopped"));
    }
    // Stopping the mockup server removes its port file, and the link.
    assert!(node(&s[2], "df-approve").links.is_empty());
    assert_eq!(status(&s[3], "df-mockup"), NodeStatus::Stale);
    let feedback = s[3]
        .loops
        .iter()
        .find(|l| l.label.as_deref() == Some("feedback"));
    assert_eq!(feedback.map(|l| l.count), Some(1));
}

#[test]
fn a_consensus_run_says_when_a_round_needs_you() {
    let s = Case {
        case: "consensus",
        variant: "patched",
        task: "t-20261101-example-consensus",
        steps: vec![
            Around::new("2026-11-01T17:06:00Z").lead("Add rate limits", AgentState::Busy, None),
            Around::new("2026-11-01T17:13:00Z").lead("Add rate limits", AgentState::Busy, None),
        ],
    }
    .derive_all();
    let design = node(&s[0], "cr-design");
    assert_eq!(design.status, NodeStatus::WaitingHuman);
    assert_eq!(design.note.as_deref(), Some("round limit reached"));
    assert!(badges(&s[0], "cr-design").contains(&"round 3/3".to_owned()));
    let code = node(&s[1], "cr-code");
    assert_eq!(code.status, NodeStatus::WaitingHuman);
    assert_eq!(code.note.as_deref(), Some("escalation"));
    assert!(badges(&s[1], "cr-code").contains(&"round 2/5".to_owned()));
    assert_eq!(node(&s[1], "df-testing").note.as_deref(), Some("declined"));
    assert_eq!(status(&s[1], "df-mockup"), NodeStatus::Omitted);
}

#[test]
fn an_old_run_without_the_newer_fields_still_reads() {
    let s = Case {
        case: "old",
        variant: "hand",
        task: "t-20260601-example-old",
        steps: vec![Around::new("2026-06-01T12:40:00Z")],
    }
    .derive_all();
    assert_eq!(status(&s[0], "df-code-quality"), NodeStatus::Unknown);
    assert_eq!(status(&s[0], "df-implement"), NodeStatus::Done);
    assert!(badges(&s[0], "df-implement").contains(&"x2".to_owned()));
}

#[test]
fn every_standard_status_shows_up_across_the_fixtures() {
    // The other tests are rewriting the files this reads.
    if std::env::var_os("MAXHQ_REGENERATE").is_some() {
        return;
    }
    let mut seen = BTreeSet::new();
    for dir in std::fs::read_dir(fixtures().join("runs")).unwrap() {
        for variant in std::fs::read_dir(dir.unwrap().path()).unwrap() {
            for file in std::fs::read_dir(variant.unwrap().path()).unwrap() {
                let path = file.unwrap().path();
                if path.extension().is_some_and(|x| x == "json") {
                    let v: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    for n in v["snapshot"]["nodes"].as_array().unwrap() {
                        seen.insert(n["status"].as_str().unwrap().to_owned());
                    }
                }
            }
        }
    }
    let all = [
        "omitted",
        "queued",
        "ready",
        "busy",
        "waiting_child",
        "waiting_human",
        "done",
        "skipped",
        "will_skip",
        "failed",
        "blocked",
        "stale",
        "unknown",
    ];
    let missing: Vec<_> = all.iter().filter(|s| !seen.contains(**s)).collect();
    assert!(missing.is_empty(), "no fixture shows {missing:?}");
}

#[test]
fn a_task_file_caught_mid_write_keeps_the_last_good_state() {
    // Plain devflow rewrites task.yaml in place, so a reader can find it
    // empty. The patch writes it atomically instead.
    let case = feature("plain");
    let mut files = project();
    let mut held = BTreeSet::new();
    case.mount(&mut files, 2, &mut held);
    let scan = Catalog::default().scan(&files, Path::new(PROJECT));
    let workflow = scan.workflow("devflow").unwrap();
    let found = scan.runs(&files).remove(0);
    let mut run = Run::new(
        DeviceId::derive("devbox"),
        ProjectId::derive("example-app"),
        found,
    );
    run.set_retry_pause(Duration::ZERO);
    let agents = &case.steps[1].agents;
    let good = run.derive(&workflow.descriptor, &files, agents);
    assert!(good.notice.is_none());

    files.put(case.run_dir().join("task.yaml"), "");
    let caught = run.derive(&workflow.descriptor, &files, agents);
    assert_eq!(caught.nodes, good.nodes);
    let notice = caught.notice.expect("a notice");
    assert_eq!(
        notice.text,
        "The run file doesn't read, so this is its last good state."
    );
}

#[test]
fn a_node_detail_lists_its_block_manifest_and_journal() {
    let case = feature("patched");
    let mut files = project();
    let mut held = BTreeSet::new();
    case.mount(&mut files, 3, &mut held);
    let scan = Catalog::default().scan(&files, Path::new(PROJECT));
    let workflow = scan.workflow("devflow").unwrap();
    let found = scan.runs(&files).remove(0);
    let mut run = Run::new(
        DeviceId::derive("devbox"),
        ProjectId::derive("example-app"),
        found,
    );
    let _ = run.derive(&workflow.descriptor, &files, &case.steps[2].agents);
    let detail = run.detail(&workflow.descriptor, &files, "df-research");
    let lists: Vec<(&str, Vec<&str>)> = detail
        .lists
        .iter()
        .map(|l| {
            (
                l.label.as_str(),
                l.items.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        lists,
        [
            ("Reads", vec!["requirements.md"]),
            ("Writes", vec!["research.md"]),
            (
                "Journal",
                vec![
                    "- 2026-11-01T09:05:00Z df-research: in_progress.",
                    "- 2026-11-01T09:07:00Z df-research: completed.",
                ]
            ),
        ]
    );
}

#[test]
fn a_key_the_standard_lacks_does_not_load_quietly() {
    // The first devflow proposal tested its review frame link with
    // `when: { alive_pid: ... }`. Predicates have no such key, so the link
    // showed with the server stopped and nothing said why.
    let text = std::fs::read_to_string(fixtures().join("devflow.yaml"))
        .unwrap()
        .replace(
            "    value: { match: '^(\\d+)$', in: mockup/server.port, pick: last, group: 1 }\n",
            "    value: { match: '^(\\d+)$', in: mockup/server.port, pick: last, group: 1 }\n    when: { alive_pid: mockup/server.pid }\n",
        );
    assert!(text.contains("alive_pid: mockup"));
    assert!(maxhq_workflow::load(text.as_bytes()).is_err());
}
