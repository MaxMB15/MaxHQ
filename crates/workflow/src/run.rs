// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! One run, followed over time: its files, its history and its agents
//! turned into the snapshot the canvas draws.

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use maxhq_protocol::ids::{AgentId, DeviceId, ProjectId};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::workflow::{
    AttemptSpan, Badge, DetailList, EdgeShape, GroupShape, Link, LoopState, NodeDetail, NodeShape,
    NodeState, NodeStatus, Progress, RunHeader, RunKey, RunNotice, RunShape, RunSnapshot, Tone,
    WorkflowRun,
};
use serde_json::{json, Map, Value as Json};
use sha2::{Digest, Sha256};

use crate::bind::{bind, lead_state, Claims};
use crate::catalog::RunRef;
use crate::descriptor::{
    Descriptor, EventSource, Expr, Label, LoopRule, NodeInfo, Nodes, OneOrMany, Provisional, Query,
};
use crate::eval::{
    self, eval, holds, parse_doc, read_capped, render, Env, NodeRow, RunFacts, Scope,
};
use crate::events::{first_existing, map_status, Event, Format, History, Log};
use crate::inputs::{AgentFacts, AgentState, Agents, Files};
use crate::load::local_url;

/// How long a live agent may go without writing before its node is quiet.
const QUIET: Duration = Duration::from_secs(10 * 60);
/// How often a run file that doesn't parse is read again before giving up.
const RETRIES: u32 = 3;

/// Where a run's state comes from.
#[derive(Clone, Debug)]
enum Origin {
    /// A run file a descriptor's glob matched.
    File(RunRef),
    /// A lead and its sub-agents, for `maxhq.agents`.
    Lead {
        project: PathBuf,
        lead: AgentId,
        /// Every sub-agent seen so far, as last seen, so one that ended
        /// stays in the diagram after it leaves the agent list.
        subs: Vec<AgentFacts>,
    },
}

/// One run, followed from derive to derive.
#[derive(Debug)]
pub struct Run {
    device: DeviceId,
    project_id: ProjectId,
    origin: Origin,
    notice: Option<RunNotice>,
    retry_pause: Duration,
    log: Option<Log>,
    history: History,
    /// Statuses at the last derive, for the history kept with no log.
    seen: HashMap<String, NodeStatus>,
    doc: Option<Json>,
    facts: RunFacts,
    rows: Vec<NodeRow>,
    orchestrator: AgentState,
    orchestrators: Vec<AgentId>,
    /// Every agent bound to each node so far, with when it started, so an
    /// attempt keeps its agent after the agent leaves.
    met: HashMap<String, Vec<(AgentId, Timestamp)>>,
    header: Vec<Badge>,
    last_activity: Option<Timestamp>,
    last: Option<RunSnapshot>,
}

impl Run {
    /// A run read from its file.
    pub fn new(device: DeviceId, project_id: ProjectId, run: RunRef) -> Self {
        Self::with(device, project_id, Origin::File(run))
    }

    /// A lead and its sub-agents, drawn by `maxhq.agents`.
    pub fn lead(device: DeviceId, project_id: ProjectId, project: &Path, lead: AgentId) -> Self {
        Self::with(
            device,
            project_id,
            Origin::Lead {
                project: project.to_owned(),
                lead,
                subs: Vec::new(),
            },
        )
    }

    fn with(device: DeviceId, project_id: ProjectId, origin: Origin) -> Self {
        Self {
            device,
            project_id,
            origin,
            notice: None,
            retry_pause: Duration::from_millis(20),
            log: None,
            history: History::default(),
            seen: HashMap::new(),
            doc: None,
            facts: RunFacts::default(),
            rows: Vec::new(),
            orchestrator: AgentState::Gone,
            orchestrators: Vec::new(),
            met: HashMap::new(),
            header: Vec::new(),
            last_activity: None,
            last: None,
        }
    }

    /// A notice to show when reading went fine, such as why this run is
    /// drawn by the fallback.
    pub fn set_notice(&mut self, notice: Option<RunNotice>) {
        self.notice = notice;
    }

    /// How long to wait before reading a run file that didn't parse again.
    pub fn set_retry_pause(&mut self, pause: Duration) {
        self.retry_pause = pause;
    }

    /// The run file, for a run read from one.
    pub fn file(&self) -> Option<&Path> {
        match &self.origin {
            Origin::File(run) => Some(&run.file),
            Origin::Lead { .. } => None,
        }
    }

    /// The agents that lead the run, as of the last derive.
    pub fn orchestrators(&self) -> &[AgentId] {
        &self.orchestrators
    }

    /// The descriptor's header lines, as of the last derive.
    pub fn header_lines(&self) -> &[Badge] {
        &self.header
    }

    /// The run's history, oldest first.
    pub fn events(&self) -> &[Event] {
        self.history.events()
    }

    /// Reads the run again and says where it stands. `rev` counts up only
    /// when something changed.
    pub fn derive(
        &mut self,
        desc: &Descriptor,
        files: &dyn Files,
        agents: &dyn Agents,
    ) -> RunSnapshot {
        let facts = agents.list();
        let now = agents.now();
        let (doc, problem) = self.read(files, &facts);
        let (project, root, run_dir, run_file) = self.places();
        let scope = Scope::new(files, &project, &root, &run_dir, run_file.as_deref(), &doc);
        let (lead, status_word): (Option<AgentId>, Option<String>);

        let (mut rows, cut) = node_rows(&desc.nodes, &doc);
        fn at<'a>(
            scope: &'a Scope<'a>,
            nodes: &'a Nodes,
            run: &'a RunFacts,
            rows: &'a [NodeRow],
            orchestrator: AgentState,
        ) -> Env<'a> {
            Env {
                scope,
                nodes_map: nodes,
                run,
                rows,
                orchestrator,
                node: None,
                vars: &[],
                value: None,
            }
        }
        let run = {
            let nothing = RunFacts::default();
            let base = at(&scope, &desc.nodes, &nothing, &rows, AgentState::Gone);
            let read = |expr: &Option<Expr>, path: &str| match expr {
                Some(expr) => eval(expr, &base),
                None => eval(&reference(path), &base),
            };
            let id = read(&desc.runs.id, "$.run.id")
                .as_ref()
                .and_then(eval::text)
                .unwrap_or_else(|| folder_name(&run_dir));
            let label = read(&desc.runs.label, "$.run.label")
                .as_ref()
                .and_then(eval::text);
            lead = read(&desc.runs.agent, "$.run.agent")
                .as_ref()
                .and_then(eval::text)
                .and_then(|a| a.parse().ok());
            status_word = read(&desc.runs.status, "$.run.status")
                .as_ref()
                .and_then(eval::text);
            RunFacts {
                label: label.unwrap_or_else(|| id.clone()),
                id,
                facts: desc
                    .runs
                    .facts
                    .iter()
                    .filter_map(|(name, expr)| Some((name.to_owned(), eval(expr, &base)?)))
                    .collect(),
            }
        };

        self.follow(desc, files, &run_dir, &rows, now);

        let node_ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
        let mut node_agents: Vec<(String, AgentId)> = rows
            .iter()
            .filter_map(|r| {
                let agent = r.field(&desc.nodes, "agent")?;
                Some((r.id.clone(), eval::text(&agent)?.parse().ok()?))
            })
            .collect();
        node_agents.extend(
            self.history
                .events()
                .iter()
                .filter_map(|e| Some((e.node.clone(), e.agent?))),
        );
        let mut node_pids = Vec::new();
        let base = at(&scope, &desc.nodes, &run, &rows, AgentState::Gone);
        for pattern in desc.bind.iter().flat_map(|b| &b.process) {
            for row in &rows {
                let pid = eval(&pattern.pid, &base.with_node(Some(row)));
                if let Some(pid) = pid
                    .as_ref()
                    .and_then(eval::text)
                    .and_then(|p| p.trim().parse().ok())
                {
                    node_pids.push((row.id.clone(), pid));
                }
            }
        }
        let claims = Claims {
            run: &run.id,
            lead,
            node_agents,
            node_pids,
            node_ids: &node_ids,
        };
        let binding = bind(desc.bind.as_ref(), &claims, &facts, agents);
        let orchestrator = lead_state(&binding.orchestrators, &facts);
        let mut bound: HashMap<String, Vec<AgentId>> = HashMap::new();
        for row in &rows {
            let mut list = binding.nodes.get(&row.id).cloned().unwrap_or_default();
            if row.info.as_ref().is_some_and(|i| i.host) && started(row.status) {
                for id in &binding.orchestrators {
                    if !list.contains(id) {
                        list.push(*id);
                    }
                }
            }
            let met = self.met.entry(row.id.clone()).or_default();
            for id in &list {
                let start = facts.iter().find(|f| f.id == *id).and_then(|f| f.started);
                if let Some(start) = start.filter(|_| !met.iter().any(|(m, _)| m == id)) {
                    met.push((*id, start));
                }
            }
            bound.insert(row.id.clone(), list);
        }

        let quiet: Vec<bool> = rows
            .iter()
            .map(|row| is_quiet(&bound[&row.id], &facts, now))
            .collect();
        standard_rules(&mut rows, &bound, &facts, orchestrator, agents.binds());

        let env = at(&scope, &desc.nodes, &run, &rows, orchestrator);
        let notes = last_notes(self.history.events());
        let mut nodes = Vec::with_capacity(rows.len());
        for (row, quiet) in rows.iter().zip(quiet) {
            let env = env.with_node(Some(row));
            let ruled = apply_rules(desc, &env);
            let agents_here = bound[&row.id].clone();
            nodes.push(NodeState {
                id: row.id.clone(),
                status: ruled.status.unwrap_or(row.status),
                provisional: ruled.provisional,
                note: ruled
                    .note
                    .or_else(|| notes.get(row.id.as_str()).map(|n| (*n).to_owned())),
                badges: badges(desc, &env),
                attempts: spans(self.history.events(), &row.id, &self.met[&row.id]),
                agents: agents_here,
                quiet,
                links: links(desc, &env),
            });
        }
        for (row, node) in rows.iter_mut().zip(&nodes) {
            row.status = node.status;
        }

        self.header = desc
            .header
            .iter()
            .filter_map(|l| label_badge(l, &at(&scope, &desc.nodes, &run, &rows, orchestrator)))
            .collect();
        let shape = shape(desc, &rows);
        let loops = desc
            .loops
            .iter()
            .filter_map(|l| loop_state(l, self.history.events(), &rows))
            .collect();
        let status = status_word.map_or_else(
            || run_status(&nodes, orchestrator),
            |w| map_status(&desc.nodes, &w),
        );
        let counted = nodes.iter().filter(|n| n.status != NodeStatus::Omitted);
        let header = RunHeader {
            label: run.label.clone(),
            status,
            progress: Progress {
                done: counted
                    .clone()
                    .filter(|n| matches!(n.status, NodeStatus::Done | NodeStatus::Skipped))
                    .count() as u32,
                total: counted.count() as u32,
            },
            lines: self.header.clone(),
        };
        let notice = problem
            .or_else(|| self.notice.clone())
            .or_else(|| {
                (cut || scope.gave_up()).then(|| RunNotice {
                    text: TOO_MUCH.to_owned(),
                    tone: Tone::Warn,
                })
            })
            .or_else(|| {
                desc.newer().then(|| RunNotice {
                    text: "Made for a newer MaxHQ".to_owned(),
                    tone: Tone::Info,
                })
            });
        let modified = run_file
            .as_deref()
            .and_then(|f| files.stat(f).ok())
            .map(|s| s.modified);
        self.last_activity = [self.last_activity, modified, self.history.last_time()]
            .into_iter()
            .flatten()
            .max();

        let mut snapshot = RunSnapshot {
            key: RunKey {
                device: self.device,
                project: self.project_id,
                workflow: desc.id.clone(),
                run: run.id.clone(),
            },
            rev: 1,
            shape,
            nodes,
            loops,
            header,
            notice,
        };
        if let Some(last) = &self.last {
            snapshot.rev = last.rev;
            if !same_content(last, &snapshot) {
                snapshot.rev += 1;
            }
        }
        self.doc = Some(doc);
        self.facts = run;
        self.rows = rows;
        self.orchestrator = orchestrator;
        self.orchestrators = binding.orchestrators;
        self.last = Some(snapshot.clone());
        snapshot
    }

    /// The run as the view lists it, as of the last derive.
    pub fn summary(&self) -> Option<WorkflowRun> {
        let last = self.last.as_ref()?;
        Some(summarize(
            last,
            self.orchestrator == AgentState::Waiting,
            self.orchestrators.clone(),
            self.last_activity,
        ))
    }

    /// The descriptor's lists for one node, read now from the files the
    /// last derive saw.
    pub fn detail(&self, desc: &Descriptor, files: &dyn Files, node: &str) -> NodeDetail {
        let (Some(doc), Some(row)) = (&self.doc, self.rows.iter().find(|r| r.id == node)) else {
            return NodeDetail { lists: Vec::new() };
        };
        let (project, root, run_dir, run_file) = self.places();
        let scope = Scope::new(files, &project, &root, &run_dir, run_file.as_deref(), doc);
        let env = Env {
            scope: &scope,
            nodes_map: &desc.nodes,
            run: &self.facts,
            rows: &self.rows,
            orchestrator: self.orchestrator,
            node: Some(row),
            vars: &[],
            value: None,
        };
        let lists = desc
            .detail
            .iter()
            .map(|rule| {
                let mut items: Vec<String> = match eval(&rule.list, &env) {
                    Some(Json::Array(items)) => items.iter().filter_map(eval::text).collect(),
                    Some(one) => eval::text(&one).into_iter().collect(),
                    None => Vec::new(),
                };
                if let Some(limit) = rule.limit {
                    items.drain(..items.len().saturating_sub(limit));
                }
                DetailList {
                    label: rule.label.clone(),
                    items,
                }
            })
            .collect();
        NodeDetail { lists }
    }

    /// The project, the workflow folder, the run folder and the run file.
    fn places(&self) -> (PathBuf, PathBuf, PathBuf, Option<PathBuf>) {
        match &self.origin {
            Origin::File(run) => {
                let dir = run
                    .file
                    .parent()
                    .map_or_else(|| run.project.clone(), Path::to_owned);
                (
                    run.project.clone(),
                    run.root.clone(),
                    dir,
                    Some(run.file.clone()),
                )
            }
            Origin::Lead { project, .. } => {
                (project.clone(), project.clone(), project.clone(), None)
            }
        }
    }

    /// The run file, parsed, or the last good one with a notice when it
    /// doesn't parse after a few tries.
    fn read(&mut self, files: &dyn Files, facts: &[AgentFacts]) -> (Json, Option<RunNotice>) {
        let file = match &mut self.origin {
            Origin::Lead { lead, subs, .. } => {
                remember_subs(*lead, subs, facts);
                return (agents_doc(*lead, facts, subs), None);
            }
            Origin::File(run) => run.file.clone(),
        };
        for attempt in 0..=RETRIES {
            if attempt > 0 && !self.retry_pause.is_zero() {
                std::thread::sleep(self.retry_pause);
            }
            if let Some(doc) = read_capped(files, &file).and_then(|bytes| parse_doc(&bytes)) {
                return (doc, None);
            }
        }
        let notice = RunNotice {
            text: "The run file doesn't read, so this is its last good state.".to_owned(),
            tone: Tone::Warn,
        };
        (self.doc.clone().unwrap_or_else(|| json!({})), Some(notice))
    }

    /// Reads the event log, or, with none, records status changes since the
    /// last derive.
    fn follow(
        &mut self,
        desc: &Descriptor,
        files: &dyn Files,
        run_dir: &Path,
        rows: &[NodeRow],
        now: Timestamp,
    ) {
        if self.log.is_none() && !desc.runs.events.is_empty() {
            let logs = desc
                .runs
                .events
                .iter()
                .map(|source| match source {
                    EventSource::Jsonl { jsonl } => (run_dir.join(jsonl), Format::Jsonl),
                    EventSource::Lines { lines, pattern } => {
                        (run_dir.join(lines), Format::Lines(pattern.0.clone()))
                    }
                })
                .filter(|(path, _)| eval::normalize(path).is_some_and(|p| p.starts_with(run_dir)))
                .collect();
            self.log = first_existing(files, logs);
            if self.log.is_some() {
                self.history.clear();
            }
        }
        if let Some(log) = &mut self.log {
            log.poll(files, &desc.nodes, &mut self.history);
            return;
        }
        let first = self.seen.is_empty();
        for row in rows {
            let before = self.seen.insert(row.id.clone(), row.status);
            let changed = if first {
                active(row.status)
            } else {
                before != Some(row.status)
            };
            if changed {
                self.history.push(Event {
                    t: now,
                    node: row.id.clone(),
                    status: Some(row.status),
                    note: None,
                    attempt: None,
                    agent: None,
                });
            }
        }
    }
}

/// A snapshot as the view lists it, for a run followed elsewhere too.
/// `lead_waiting` says whether the run's lead waits for you.
pub fn summarize(
    last: &RunSnapshot,
    lead_waiting: bool,
    orchestrators: Vec<AgentId>,
    last_activity: Option<Timestamp>,
) -> WorkflowRun {
    let status = |s: NodeStatus| last.nodes.iter().filter(move |n| n.status == s);
    let done = matches!(
        last.header.status,
        NodeStatus::Done | NodeStatus::Failed | NodeStatus::Skipped
    );
    let mut agents: Vec<AgentId> = Vec::new();
    for agent in last.nodes.iter().flat_map(|n| &n.agents) {
        if !orchestrators.contains(agent) && !agents.contains(agent) {
            agents.push(*agent);
        }
    }
    WorkflowRun {
        id: last.key.id(),
        key: last.key.clone(),
        label: last.header.label.clone(),
        progress: last.header.progress,
        current: status(NodeStatus::Busy).map(|n| n.id.clone()).collect(),
        needs_you: status(NodeStatus::WaitingHuman).next().is_some() || (lead_waiting && !done),
        orchestrators,
        last_activity,
        done,
        failed: last.header.status == NodeStatus::Failed,
        agents,
    }
}

/// A value source for one place in the reference shape.
fn reference(path: &str) -> Expr {
    Expr::Source(Box::new(crate::descriptor::Source {
        path: Some(Query::parse(path).expect("a valid path")),
        ..Default::default()
    }))
}

fn folder_name(dir: &Path) -> String {
    dir.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

fn active(status: NodeStatus) -> bool {
    matches!(
        status,
        NodeStatus::Busy | NodeStatus::WaitingChild | NodeStatus::WaitingHuman
    )
}

/// Whether work on a node has begun, so a host node's lead is on it.
fn started(status: NodeStatus) -> bool {
    !matches!(
        status,
        NodeStatus::Omitted
            | NodeStatus::Queued
            | NodeStatus::Ready
            | NodeStatus::WillSkip
            | NodeStatus::Skipped
            | NodeStatus::Blocked
    )
}

/// The most nodes a run shows. A guest's run stops well below this.
const MAX_ROWS: usize = 2000;

/// What a run says when it's too big to read in full.
const TOO_MUCH: &str = "This workflow is too big to read in full, so what it shows may be wrong.";

/// The run file's nodes as rows, then the nodes `info` names that the run
/// doesn't list, as omitted. Also whether the list went past [`MAX_ROWS`].
fn node_rows(nodes: &Nodes, doc: &Json) -> (Vec<NodeRow>, bool) {
    let found = nodes.list.0.query(doc).all();
    let items: Vec<&Json> = match found.as_slice() {
        [Json::Array(items)] => items.iter().collect(),
        other => other.to_vec(),
    };
    let mut rows: Vec<NodeRow> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let info: HashMap<&str, &NodeInfo> = nodes.info.iter().collect();
    for item in items {
        let Some(fields) = item.as_object() else {
            continue;
        };
        let Some(id) = fields
            .get(&nodes.id)
            .and_then(eval::text)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if seen.contains(&id) {
            continue;
        }
        if rows.len() == MAX_ROWS {
            return (rows, true);
        }
        seen.insert(id.clone());
        let word = fields.get(&nodes.status).and_then(eval::text);
        let needs = match fields.get(&nodes.needs) {
            Some(Json::Array(list)) => list.iter().filter_map(eval::text).collect(),
            Some(one) => eval::text(one).into_iter().collect(),
            None => Vec::new(),
        };
        rows.push(NodeRow {
            status: word
                .as_deref()
                .map_or(NodeStatus::Queued, |w| map_status(nodes, w)),
            word,
            group: fields.get(&nodes.group).and_then(eval::text),
            needs,
            info: info.get(id.as_str()).map(|info| (*info).clone()),
            fields: fields.clone(),
            id,
        });
    }
    for (id, info) in nodes.info.iter() {
        if !seen.contains(id) {
            if rows.len() == MAX_ROWS {
                return (rows, true);
            }
            rows.push(NodeRow {
                id: id.to_owned(),
                status: NodeStatus::Omitted,
                word: None,
                fields: Map::new(),
                group: None,
                needs: Vec::new(),
                info: Some(info.clone()),
            });
        }
    }
    (rows, false)
}

/// The standard's rules, the same for every workflow. Each reads the
/// statuses as they were before any of them.
fn standard_rules(
    rows: &mut [NodeRow],
    bound: &HashMap<String, Vec<AgentId>>,
    facts: &[AgentFacts],
    orchestrator: AgentState,
    binds: bool,
) {
    let before: HashMap<String, NodeStatus> =
        rows.iter().map(|r| (r.id.clone(), r.status)).collect();
    let met = |id: &String| {
        before.get(id).is_none_or(|s| {
            matches!(
                s,
                NodeStatus::Done | NodeStatus::Skipped | NodeStatus::Omitted
            )
        })
    };
    let lead_away = matches!(orchestrator, AgentState::Idle | AgentState::Gone);
    for row in rows.iter_mut() {
        match row.status {
            NodeStatus::Queued if row.needs.iter().all(met) => row.status = NodeStatus::Ready,
            NodeStatus::Busy if binds && lead_away => {
                let alive = bound[&row.id]
                    .iter()
                    .any(|id| facts.iter().any(|f| f.id == *id && f.state.alive()));
                if !alive {
                    row.status = NodeStatus::Stale;
                }
            }
            _ => {}
        }
    }
}

/// Whether every live agent on a node has been silent a while, with no tool
/// call open.
fn is_quiet(agents: &[AgentId], facts: &[AgentFacts], now: Timestamp) -> bool {
    let live: Vec<&AgentFacts> = agents
        .iter()
        .filter_map(|id| facts.iter().find(|f| f.id == *id))
        .filter(|f| f.state.alive())
        .collect();
    let cutoff = now.0.saturating_sub(QUIET.as_millis() as u64);
    !live.is_empty()
        && live
            .iter()
            .all(|f| !f.tool_open && f.last_write.or(f.started).is_some_and(|t| t.0 <= cutoff))
}

/// What the first matching rule says about a node.
#[derive(Default)]
struct Ruled {
    status: Option<NodeStatus>,
    note: Option<String>,
    provisional: bool,
}

fn apply_rules(desc: &Descriptor, env: &Env) -> Ruled {
    for rule in &desc.rules {
        let cell = OnceCell::new();
        let env = Env {
            value: rule.value.as_ref().map(|v| (v, &cell)),
            ..*env
        };
        if !holds(&rule.when, &env) {
            continue;
        }
        return Ruled {
            status: rule.is,
            note: rule.note.as_deref().and_then(|t| fill(t, &env)),
            provisional: match &rule.provisional {
                None => false,
                Some(Provisional::Always(b)) => *b,
                Some(Provisional::When(p)) => holds(p, &env),
            },
        };
    }
    Ruled::default()
}

fn fill(template: &str, env: &Env) -> Option<String> {
    render(template, &|name| env.lookup(name), &|s| s.into())
}

fn on_node(filter: &Option<OneOrMany<String>>, env: &Env) -> bool {
    match (filter, env.node) {
        (None, _) => true,
        (Some(ids), Some(row)) => ids.contains(&row.id),
        (Some(_), None) => false,
    }
}

/// A label's text and tone when its test holds and its text fills in.
fn label_badge(label: &Label, env: &Env) -> Option<Badge> {
    let vars: Vec<(String, Json)> = label
        .vars
        .iter()
        .filter_map(|(name, expr)| Some((name.to_owned(), eval(expr, env)?)))
        .collect();
    let cell = OnceCell::new();
    let env = Env {
        vars: &vars,
        value: label.value.as_ref().map(|v| (v, &cell)),
        ..*env
    };
    if label.when.as_ref().is_some_and(|w| !holds(w, &env)) {
        return None;
    }
    Some(Badge {
        text: fill(&label.text, &env).filter(|t| !t.is_empty())?,
        tone: label.tone.unwrap_or(Tone::Neutral),
    })
}

fn badges(desc: &Descriptor, env: &Env) -> Vec<Badge> {
    desc.badges
        .iter()
        .filter(|b| on_node(&b.node, env))
        .filter_map(|b| label_badge(&b.label, env))
        .collect()
}

fn links(desc: &Descriptor, env: &Env) -> Vec<Link> {
    desc.links
        .iter()
        .filter(|l| on_node(&l.node, env))
        .filter_map(|l| {
            let cell = OnceCell::new();
            let env = Env {
                value: l.value.as_ref().map(|v| (v, &cell)),
                ..*env
            };
            if l.when.as_ref().is_some_and(|w| !holds(w, &env)) {
                return None;
            }
            let url = fill(&l.url, &env).filter(|u| local_url(u))?;
            Some(Link {
                label: l.label.clone(),
                url,
            })
        })
        .collect()
}

/// Each node's last note from the history.
fn last_notes(events: &[Event]) -> HashMap<&str, &str> {
    events
        .iter()
        .filter_map(|e| Some((e.node.as_str(), e.note.as_deref()?)))
        .collect()
}

/// A node's attempts from its history. A busy event with the same attempt,
/// or none, while one is open continues it; a new attempt number starts
/// another. The open attempt keeps when it last began waiting on you until
/// it stops. Agents pair with spans by start time, from every agent bound
/// to the node so far.
fn spans(events: &[Event], node: &str, started: &[(AgentId, Timestamp)]) -> Vec<AttemptSpan> {
    let mut spans: Vec<AttemptSpan> = Vec::new();
    let mut open: Option<Option<u64>> = None;
    let mut named: Vec<bool> = Vec::new();
    for e in events.iter().filter(|e| e.node == node) {
        let Some(status) = e.status else { continue };
        let waits = status == NodeStatus::WaitingHuman;
        if active(status) {
            let same =
                matches!(open, Some(a) if e.attempt.is_none() || a.is_none() || a == e.attempt);
            if same {
                if open.is_some_and(|a| a.is_none()) && e.attempt.is_some() {
                    open = Some(e.attempt);
                }
                if let Some(last) = spans.last_mut() {
                    last.waiting_since = if waits {
                        last.waiting_since.or(Some(e.t))
                    } else {
                        None
                    };
                }
                continue;
            }
            if let Some(last) = spans.last_mut().filter(|_| open.is_some()) {
                last.end = Some(e.t);
                last.waiting_since = None;
            }
            spans.push(AttemptSpan {
                start: e.t,
                end: None,
                end_status: None,
                agent: e.agent,
                waiting_since: waits.then_some(e.t),
            });
            named.push(e.agent.is_some());
            open = Some(e.attempt);
        } else if open.take().is_some()
            && let Some(last) = spans.last_mut()
        {
            last.end = Some(e.t);
            last.end_status = Some(status);
            last.waiting_since = None;
        }
    }
    for (span, named) in spans.iter_mut().zip(named) {
        if !named {
            span.agent = started
                .iter()
                .min_by_key(|(_, t)| t.0.abs_diff(span.start.0))
                .map(|(id, _)| *id);
        }
    }
    spans
}

/// How often work went round a loop: each time a `to` node, or a member of
/// a `to` group, goes busy again after `from` went busy. Its `to` is the
/// target it last went back to, or the first before it has. Targets the run
/// leaves out are dropped, and so is a loop with none left or no `from`.
fn loop_state(rule: &LoopRule, events: &[Event], rows: &[NodeRow]) -> Option<LoopState> {
    let in_run = |r: &&NodeRow| r.status != NodeStatus::Omitted;
    rows.iter().filter(in_run).find(|r| r.id == rule.from)?;
    let targets: Vec<&String> = rule
        .to
        .as_slice()
        .iter()
        .filter(|t| {
            rows.iter()
                .filter(in_run)
                .any(|r| r.id == **t || r.group.as_ref() == Some(*t))
        })
        .collect();
    let target_of = |node: &str| {
        targets.iter().copied().find(|t| *t == node).or_else(|| {
            let group = rows.iter().find(|r| r.id == node)?.group.as_ref()?;
            targets.iter().copied().find(|t| *t == group)
        })
    };
    let mut to = *targets.first()?;
    let mut last: HashMap<&str, NodeStatus> = HashMap::new();
    let mut started: Vec<&str> = Vec::new();
    let mut armed = false;
    let mut count = 0;
    for e in events {
        let Some(status) = e.status else { continue };
        let went =
            status == NodeStatus::Busy && last.get(e.node.as_str()) != Some(&NodeStatus::Busy);
        last.insert(&e.node, status);
        if !went {
            continue;
        }
        if e.node == rule.from {
            armed = true;
        } else if let Some(target) = target_of(&e.node) {
            if armed && started.contains(&e.node.as_str()) {
                count += 1;
                armed = false;
                to = target;
            }
            started.push(&e.node);
        }
    }
    Some(LoopState {
        from: rule.from.clone(),
        to: to.clone(),
        label: rule.label.clone(),
        count,
        budget: rule.budget,
    })
}

/// The run's status from its nodes, when the run file doesn't say. A failed
/// node fails the run only once its lead is idle or gone; while the lead
/// is busy or waiting it may still retry, so the run is busy.
fn run_status(nodes: &[NodeState], lead: AgentState) -> NodeStatus {
    let lead_on = matches!(lead, AgentState::Busy | AgentState::Waiting);
    let has = |s: NodeStatus| nodes.iter().any(|n| n.status == s);
    let counted: Vec<&NodeState> = nodes
        .iter()
        .filter(|n| n.status != NodeStatus::Omitted)
        .collect();
    if !counted.is_empty()
        && counted.iter().all(|n| {
            matches!(
                n.status,
                NodeStatus::Done | NodeStatus::Skipped | NodeStatus::WillSkip
            )
        })
    {
        NodeStatus::Done
    } else if has(NodeStatus::Failed) && !lead_on {
        NodeStatus::Failed
    } else if has(NodeStatus::WaitingHuman) {
        NodeStatus::WaitingHuman
    } else if has(NodeStatus::Busy) || has(NodeStatus::Failed) {
        NodeStatus::Busy
    } else {
        NodeStatus::Queued
    }
}

fn shape(desc: &Descriptor, rows: &[NodeRow]) -> RunShape {
    let nodes: Vec<NodeShape> = rows
        .iter()
        .map(|row| NodeShape {
            id: row.id.clone(),
            label: row
                .info
                .as_ref()
                .and_then(|i| i.label.clone())
                .or_else(|| {
                    row.field(&desc.nodes, "label")
                        .as_ref()
                        .and_then(eval::text)
                })
                .unwrap_or_else(|| row.id.clone()),
            group: row.group.clone(),
            lane: row.info.as_ref().and_then(|i| i.lane.clone()),
            host: None,
        })
        .collect();
    let edges = rows
        .iter()
        .flat_map(|row| {
            row.needs
                .iter()
                .filter(|need| rows.iter().any(|r| r.id == **need))
                .map(|need| EdgeShape {
                    from: need.clone(),
                    to: row.id.clone(),
                })
        })
        .collect();
    let mut groups: Vec<GroupShape> = Vec::new();
    for id in rows.iter().filter_map(|r| r.group.as_ref()) {
        if !groups.iter().any(|g| g.id == *id) {
            let info = desc.groups.get(id);
            groups.push(GroupShape {
                id: id.clone(),
                label: info
                    .and_then(|i| i.label.clone())
                    .unwrap_or_else(|| id.clone()),
                collapsed: info.is_some_and(|i| i.collapsed),
            });
        }
    }
    let mut shape = RunShape {
        nodes,
        edges,
        groups,
        hash: String::new(),
    };
    let bytes =
        serde_json::to_vec(&(&shape.nodes, &shape.edges, &shape.groups)).expect("shapes serialize");
    let digest = Sha256::digest(&bytes);
    shape.hash = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    shape
}

/// Whether two snapshots say the same, whatever their revisions.
fn same_content(a: &RunSnapshot, b: &RunSnapshot) -> bool {
    a.key == b.key
        && a.shape == b.shape
        && a.nodes == b.nodes
        && a.loops == b.loops
        && a.header == b.header
        && a.notice == b.notice
}

/// A run file in the reference shape for a lead and its sub-agents.
/// Folds the lead's sub-agents in `facts` into `subs`: a new one joins at
/// the end, a known one takes its latest facts, and one no longer listed
/// keeps its last facts but reads as gone. Capped at [`MAX_ROWS`].
fn remember_subs(lead: AgentId, subs: &mut Vec<AgentFacts>, facts: &[AgentFacts]) {
    for sub in subs.iter_mut() {
        match facts.iter().find(|f| f.id == sub.id) {
            Some(now) => {
                let title = sub.title.take();
                *sub = now.clone();
                // A title once known stays when a later read lacks it.
                sub.title = sub.title.take().or(title);
            }
            None => sub.state = AgentState::Gone,
        }
    }
    for f in facts.iter().filter(|f| f.lead == Some(lead)) {
        if subs.len() < MAX_ROWS && !subs.iter().any(|s| s.id == f.id) {
            subs.push(f.clone());
        }
    }
}

fn agents_doc(lead: AgentId, facts: &[AgentFacts], subs: &[AgentFacts]) -> Json {
    let word = |state: AgentState| match state {
        AgentState::Busy => "busy",
        AgentState::Waiting => "waiting_human",
        AgentState::Idle | AgentState::Gone => "done",
    };
    let me = facts.iter().find(|f| f.id == lead);
    let mut subs: Vec<&AgentFacts> = subs.iter().collect();
    subs.sort_by_key(|f| f.started);
    let nodes: Vec<Json> = subs
        .iter()
        .map(|f| {
            json!({
                "id": f.id.to_string(),
                "status": word(f.state),
                "label": f.title.clone().unwrap_or_else(|| "Sub-agent".to_owned()),
                "agent": f.id.to_string(),
            })
        })
        .collect();
    json!({
        "run": {
            "id": lead.to_string(),
            "label": me.and_then(|f| f.title.clone()).unwrap_or_else(|| "Agent".to_owned()),
            "agent": lead.to_string(),
        },
        "nodes": nodes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeAgents, MemFiles};
    use crate::load::load;

    const PROJECT: &str = "/work/example-app";
    const FILE: &str = "/work/example-app/.runs/r1/run.yaml";
    const LOG: &str = "/work/example-app/.runs/r1/events.jsonl";

    fn descriptor(extra: &str) -> Descriptor {
        let text = format!(
            "format: maxhq.workflow/1.0\nid: example\nruns:\n  glob: .runs/*/run.yaml\n{extra}"
        );
        load(text.as_bytes()).unwrap()
    }

    fn run_ref() -> RunRef {
        RunRef {
            workflow: "example".into(),
            file: FILE.into(),
            project: PROJECT.into(),
            root: PROJECT.into(),
        }
    }

    fn new_run() -> Run {
        let mut run = Run::new(DeviceId::derive("d"), ProjectId::derive("p"), run_ref());
        run.set_retry_pause(Duration::ZERO);
        run
    }

    fn node<'a>(s: &'a RunSnapshot, id: &str) -> &'a NodeState {
        s.nodes.iter().find(|n| n.id == id).unwrap()
    }

    const REFERENCE: &str = "run: { id: r1, label: Example run }
nodes:
  - { id: plan, status: done }
  - { id: build, status: busy, needs: [plan], group: work }
  - { id: test, needs: [build], group: work }
  - { id: ship, needs: [test, missing] }
";

    fn many_nodes(count: usize) -> String {
        let nodes: Vec<String> = (0..count).map(|n| format!("{{ id: n{n} }}")).collect();
        format!("run: {{ id: r1 }}\nnodes: [{}]\n", nodes.join(", "))
    }

    #[test]
    fn a_predicate_that_multiplies_gives_up_and_says_so() {
        let mut files = MemFiles::default();
        files.put(FILE, many_nodes(1000));
        let when = "{ any_node: { any_node: { any_node: { any_node: { node: nope } } } } }";
        let desc = descriptor(&format!("rules: [{{ when: {when}, is: done }}]\n"));
        let s = new_run().derive(&desc, &files, &FakeAgents::default());
        let notice = s.notice.expect("says it gave up");
        assert_eq!(notice.text, TOO_MUCH);
        assert_eq!(notice.tone, Tone::Warn);
        assert_eq!(s.nodes.len(), 1000);
    }

    #[test]
    fn a_big_run_keeps_its_first_nodes_and_says_so() {
        let mut files = MemFiles::default();
        files.put(FILE, many_nodes(MAX_ROWS + 5));
        let s = new_run().derive(&descriptor(""), &files, &FakeAgents::default());
        assert_eq!(s.nodes.len(), MAX_ROWS);
        assert_eq!(s.nodes[0].id, "n0");
        assert_eq!(s.notice.expect("says it was cut").text, TOO_MUCH);
        // Up to the cap nothing is cut or said.
        files.put(FILE, many_nodes(MAX_ROWS));
        let s = new_run().derive(&descriptor(""), &files, &FakeAgents::default());
        assert_eq!(s.nodes.len(), MAX_ROWS);
        assert_eq!(s.notice, None);
    }

    #[test]
    fn nodes_the_run_repeats_or_info_adds_stay_once_each() {
        let mut files = MemFiles::default();
        files.put(
            FILE,
            "run: { id: r1 }\nnodes: [{ id: a }, { id: b }, { id: a }]\n",
        );
        let desc = descriptor("nodes:\n  info: { a: { label: A }, c: { label: C } }\n");
        let s = new_run().derive(&desc, &files, &FakeAgents::default());
        let ids: Vec<&str> = s.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn the_reference_shape_needs_no_mapping() {
        let mut files = MemFiles::default();
        files.put(FILE, REFERENCE);
        let mut agents = FakeAgents::default();
        let lead = agents.add("lead", None);
        agents.add("build", Some(lead));
        let s = new_run().derive(
            &descriptor("bind: { node: [{ title: '^(?<node>\\w+)$' }] }\n"),
            &files,
            &agents,
        );
        assert_eq!(s.key.run, "r1");
        assert_eq!(s.header.label, "Example run");
        assert_eq!(s.header.status, NodeStatus::Busy);
        assert_eq!(s.header.progress, Progress { done: 1, total: 4 });
        assert_eq!(
            s.shape
                .nodes
                .iter()
                .map(|n| n.id.as_str())
                .collect::<Vec<_>>(),
            ["plan", "build", "test", "ship"]
        );
        assert_eq!(
            s.shape.edges.len(),
            3,
            "an edge to a missing node is dropped"
        );
        assert_eq!(s.shape.groups[0].id, "work");
        assert_eq!(node(&s, "plan").status, NodeStatus::Done);
        assert_eq!(node(&s, "build").status, NodeStatus::Busy);
        assert_eq!(node(&s, "test").status, NodeStatus::Queued);
        assert_eq!(s.notice, None);
    }

    #[test]
    fn a_mapped_run_file_reads_through_the_mapping() {
        let mut files = MemFiles::default();
        files.put(
            FILE,
            "task: { id: t-1, title: Mapped, preset: fast }
graph:
  - { block: a, state: in_progress, depends_on: [], parallel_group: g1, tier: M }
  - { block: b, state: completed, depends_on: [a] }
",
        );
        let desc = descriptor(
            "  id: { path: $.task.id }
  label: { path: $.task.title }
  facts: { preset: { path: $.task.preset } }
nodes:
  list: $.graph
  id: block
  needs: depends_on
  group: parallel_group
  status: state
  fields: [tier]
  status_map: { in_progress: busy, completed: done }
  info:
    a: { label: Alpha, lane: side }
    c: { label: Gamma }
groups: { g1: { label: Group one, collapsed: true } }
badges:
  - { text: '{tier}', when: { field: { tier: { exists: true } } } }
  - { node: b, text: 'preset {run.preset}', tone: info }
bind: {}
",
        );
        let s = new_run().derive(&desc, &files, &FakeAgents::default());
        assert_eq!(s.key.run, "t-1");
        assert_eq!(s.header.label, "Mapped");
        let a = s.shape.nodes.iter().find(|n| n.id == "a").unwrap();
        assert_eq!(
            (a.label.as_str(), a.lane.as_deref(), a.group.as_deref()),
            ("Alpha", Some("side"), Some("g1"))
        );
        assert_eq!(s.shape.groups[0].label, "Group one");
        assert!(s.shape.groups[0].collapsed);
        assert_eq!(node(&s, "b").status, NodeStatus::Done);
        assert_eq!(
            node(&s, "c").status,
            NodeStatus::Omitted,
            "info names it, the run doesn't"
        );
        assert_eq!(s.header.progress, Progress { done: 1, total: 2 });
        assert_eq!(
            node(&s, "a").badges,
            vec![Badge {
                text: "M".into(),
                tone: Tone::Neutral
            }]
        );
        assert_eq!(
            node(&s, "b").badges,
            vec![Badge {
                text: "preset fast".into(),
                tone: Tone::Info
            }]
        );
    }

    #[test]
    fn queued_becomes_ready_when_its_needs_are_met() {
        let mut files = MemFiles::default();
        files.put(
            FILE,
            "nodes: [{ id: a, status: done }, { id: b, status: skipped }, { id: c, needs: [a, b, gone] }, { id: d, needs: [c] }]",
        );
        let s = new_run().derive(&descriptor(""), &files, &FakeAgents::default());
        assert_eq!(node(&s, "c").status, NodeStatus::Ready);
        assert_eq!(
            node(&s, "d").status,
            NodeStatus::Queued,
            "reads c as it was before the rules"
        );
    }

    #[test]
    fn busy_goes_stale_with_no_live_agent_and_an_idle_lead() {
        let mut files = MemFiles::default();
        files.put(
            FILE,
            "nodes: [{ id: a, status: busy }, { id: b, status: busy }]",
        );
        let desc = descriptor("bind: { node: [{ title: '^(?<node>\\w+)$' }] }\n");
        let mut agents = FakeAgents::default();
        let lead = agents.add("lead", None);
        let a = agents.add("a", Some(lead));
        let b = agents.add("b", Some(lead));
        agents.state(b, AgentState::Gone);
        let mut run = new_run();
        let s = run.derive(&desc, &files, &agents);
        assert_eq!(node(&s, "b").status, NodeStatus::Busy, "the lead is busy");
        agents.state(lead, AgentState::Idle);
        let s = run.derive(&desc, &files, &agents);
        assert_eq!(node(&s, "a").status, NodeStatus::Busy);
        assert_eq!(node(&s, "b").status, NodeStatus::Stale);
        agents.state(a, AgentState::Gone);
        let s = run.derive(&desc, &files, &agents);
        assert_eq!(node(&s, "a").status, NodeStatus::Stale);
        let s = new_run().derive(&desc, &files, &crate::inputs::NoAgents);
        assert_eq!(
            node(&s, "a").status,
            NodeStatus::Busy,
            "a guest leaves it to the host"
        );
    }

    #[test]
    fn a_silent_live_agent_makes_its_node_quiet() {
        let mut files = MemFiles::default();
        files.put(
            FILE,
            "nodes: [{ id: a, status: busy }, { id: b, status: busy }, { id: c, status: busy }]",
        );
        let desc = descriptor("bind: { node: [{ title: '^(?<node>\\w+)$' }] }\n");
        let mut agents = FakeAgents::default();
        agents.now = Timestamp(QUIET.as_millis() as u64 + 5_000);
        let a = agents.add("a", None);
        let b = agents.add("b", None);
        let c = agents.add("c", None);
        agents.wrote(a, 1_000, false);
        agents.wrote(b, 1_000, true);
        agents.wrote(c, 4_000 + QUIET.as_millis() as u64, false);
        let s = new_run().derive(&desc, &files, &agents);
        assert!(node(&s, "a").quiet);
        assert!(!node(&s, "b").quiet, "a tool call is open");
        assert!(!node(&s, "c").quiet);
    }

    #[test]
    fn rules_apply_first_match_with_notes_and_provisional() {
        let mut files = MemFiles::default();
        files.put(
            FILE,
            "nodes: [{ id: research, status: busy }, { id: docs, status: skip_planned }, { id: gate, status: queued, why: limit }]",
        );
        let desc = descriptor(
            "nodes:
  fields: [why]
  status_map: { skip_planned: will_skip }
rules:
  - when: { status: will_skip }
    provisional: { any_node: { id: research, status: [busy, queued, ready] } }
  - when: { node: gate, field: { why: limit } }
    is: blocked
    note: 'stopped by {why}'
  - when: { node: gate }
    is: done
",
        );
        let s = new_run().derive(&desc, &files, &crate::inputs::NoAgents);
        let docs = node(&s, "docs");
        assert_eq!(docs.status, NodeStatus::WillSkip);
        assert!(docs.provisional);
        let gate = node(&s, "gate");
        assert_eq!(gate.status, NodeStatus::Blocked);
        assert_eq!(gate.note.as_deref(), Some("stopped by limit"));
        assert!(!gate.provisional);
    }

    #[test]
    fn links_fill_in_while_their_test_holds() {
        let mut files = MemFiles::default();
        files.put(FILE, "nodes: [{ id: a, status: busy, port: 4100 }]");
        files.put("/work/example-app/.runs/r1/server.pid", "77");
        let desc = descriptor(
            "nodes: { fields: [port] }
links:
  - { label: Preview, url: 'http://127.0.0.1:{port}/' }
  - { label: Server, url: 'http://localhost:{port}/', value: { alive_pid: server.pid }, when: { value: true } }
",
        );
        let s = new_run().derive(&desc, &files, &FakeAgents::default());
        assert_eq!(
            node(&s, "a").links,
            vec![Link {
                label: "Preview".into(),
                url: "http://127.0.0.1:4100/".into()
            }]
        );
    }

    fn event(t: &str, node: &str, status: &str, attempt: Option<u64>) -> String {
        let attempt = attempt.map_or(String::new(), |a| format!(",\"attempt\":{a}"));
        format!("{{\"t\":\"2026-11-01T14:{t}Z\",\"node\":\"{node}\",\"status\":\"{status}\"{attempt}}}\n")
    }

    #[test]
    fn loops_count_from_events() {
        let mut files = MemFiles::default();
        files.put(
            FILE,
            "nodes: [{ id: build, group: work }, { id: lint, group: work }, { id: review }]",
        );
        let log: String = [
            event("00:00", "build", "busy", None),
            event("01:00", "build", "done", None),
            event("02:00", "review", "busy", None),
            event("03:00", "lint", "busy", None),
            event("04:00", "lint", "done", None),
            event("05:00", "review", "done", None),
            event("06:00", "review", "busy", None),
            event("07:00", "build", "busy", None),
            event("08:00", "build", "busy", None),
            event("09:00", "build", "done", None),
            event("09:30", "review", "done", None),
            event("10:00", "review", "busy", None),
            event("11:00", "lint", "busy", None),
        ]
        .concat();
        files.put(LOG, log);
        let desc = descriptor(
            "  events: [{ jsonl: events.jsonl }]
loops:
  - { from: review, to: work, label: review, budget: 3 }
  - { from: review, to: [build] }
",
        );
        let s = new_run().derive(&desc, &files, &FakeAgents::default());
        assert_eq!(s.loops[0].count, 2, "build again, then lint again");
        assert_eq!(s.loops[0].to, "work");
        assert_eq!(s.loops[0].budget, Some(3));
        assert_eq!(s.loops[1].count, 1, "a chunk advance isn't a new start");
    }

    #[test]
    fn attempts_come_from_events_and_pair_with_agents() {
        let mut files = MemFiles::default();
        files.put(FILE, "nodes: [{ id: test, status: busy }]");
        let log: String = [
            event("00:00", "test", "busy", Some(1)),
            event("01:00", "test", "busy", Some(1)),
            event("02:00", "test", "failed", Some(1)),
            event("03:00", "test", "busy", Some(2)),
            event("04:00", "test", "busy", Some(3)),
        ]
        .concat();
        files.put(LOG, log);
        let desc = descriptor(
            "  events: [{ jsonl: events.jsonl }]\nbind: { node: [{ title: '^(?<node>\\w+)$' }] }\n",
        );
        let mut agents = FakeAgents::default();
        let first = agents.add("test", None);
        let second = agents.add("test", None);
        let base = crate::events::parse_time("2026-11-01T14:00:00Z").unwrap().0;
        agents.started(first, base + 1);
        agents.started(second, base + 180_000);
        let s = new_run().derive(&desc, &files, &agents);
        let spans = &node(&s, "test").attempts;
        assert_eq!(
            spans.len(),
            3,
            "the same attempt continues, a new one starts"
        );
        assert_eq!(spans[0].end_status, Some(NodeStatus::Failed));
        assert_eq!(spans[0].agent, Some(first));
        assert_eq!(spans[1].agent, Some(second));
        assert!(spans[1].end.is_some() && spans[1].end_status.is_none());
        assert_eq!(spans[2].end, None);
    }

    #[test]
    fn an_attempt_keeps_when_it_began_waiting_on_you() {
        let mut files = MemFiles::default();
        files.put(FILE, "nodes: [{ id: approve }]");
        let desc = descriptor("  events: [{ jsonl: events.jsonl }]\n");
        let at = |t: &str| crate::events::parse_time(&format!("2026-11-01T14:{t}Z")).unwrap();
        let mut since = |log: &[String]| {
            files.put(LOG, log.concat());
            let s = new_run().derive(&desc, &files, &FakeAgents::default());
            node(&s, "approve").attempts.last().unwrap().waiting_since
        };
        let mut log = vec![
            event("00:00", "approve", "busy", Some(1)),
            event("02:00", "approve", "waiting_human", Some(1)),
            event("03:00", "approve", "waiting_human", Some(1)),
        ];
        assert_eq!(
            since(&log),
            Some(at("02:00")),
            "the wait's start, not the attempt's"
        );
        log.push(event("04:00", "approve", "busy", Some(1)));
        assert_eq!(since(&log), None, "leaving the wait clears it");
        log.push(event("05:00", "approve", "waiting_human", None));
        assert_eq!(since(&log), Some(at("05:00")), "a new wait takes its time");
        log.push(event("06:00", "approve", "done", Some(1)));
        assert_eq!(since(&log), None, "an ended attempt waits on no one");
    }

    #[test]
    fn a_run_file_with_no_times_waits_since_none() {
        let mut files = MemFiles::default();
        files.put(FILE, "nodes: [{ id: approve, status: waiting_human }]");
        files.put(LOG, event("00:00", "other", "busy", None));
        let desc = descriptor("  events: [{ jsonl: events.jsonl }]\n");
        let s = new_run().derive(&desc, &files, &FakeAgents::default());
        let approve = node(&s, "approve");
        assert_eq!(approve.status, NodeStatus::WaitingHuman);
        assert!(approve.attempts.is_empty(), "the log has no times for it");
    }

    #[test]
    fn loops_name_the_target_they_went_back_to_and_drop_left_out_nodes() {
        let mut files = MemFiles::default();
        files.put(FILE, "nodes: [{ id: build }, { id: lint }, { id: review }]");
        let log: String = [
            event("00:00", "build", "busy", None),
            event("01:00", "lint", "busy", None),
            event("01:30", "lint", "done", None),
            event("02:00", "review", "busy", None),
            event("03:00", "lint", "busy", None),
        ]
        .concat();
        files.put(LOG, log);
        let desc = descriptor(
            "  events: [{ jsonl: events.jsonl }]
nodes: { info: { docs: { label: Docs } } }
loops:
  - { from: review, to: [docs, build, lint] }
  - { from: review, to: docs }
  - { from: docs, to: build }
",
        );
        let s = new_run().derive(&desc, &files, &FakeAgents::default());
        assert_eq!(s.loops.len(), 1, "loops with only left out ends drop");
        assert_eq!(s.loops[0].count, 1);
        assert_eq!(s.loops[0].to, "lint", "the target it went back to");
    }

    #[test]
    fn an_attempt_keeps_its_agent_after_the_agent_leaves() {
        let mut files = MemFiles::default();
        files.put(FILE, "nodes: [{ id: test, status: busy }]");
        let log: String = [
            event("00:00", "test", "busy", Some(1)),
            event("03:00", "test", "busy", Some(2)),
        ]
        .concat();
        files.put(LOG, log);
        let desc = descriptor(
            "  events: [{ jsonl: events.jsonl }]\nbind: { node: [{ title: '^(?<node>\\w+)$' }] }\n",
        );
        let mut agents = FakeAgents::default();
        let first = agents.add("test", None);
        let second = agents.add("test", None);
        let base = crate::events::parse_time("2026-11-01T14:00:00Z").unwrap().0;
        agents.started(first, base + 1);
        agents.started(second, base + 180_000);
        let mut run = new_run();
        run.derive(&desc, &files, &agents);
        agents.remove(first);
        let s = run.derive(&desc, &files, &agents);
        let test = node(&s, "test");
        assert_eq!(test.agents, vec![second]);
        assert_eq!(test.attempts[0].agent, Some(first));
        assert_eq!(test.attempts[1].agent, Some(second));
    }

    #[test]
    fn a_failed_node_fails_the_run_only_once_the_lead_stops() {
        let mut files = MemFiles::default();
        let mut agents = FakeAgents::default();
        let lead = agents.add("lead", None);
        files.put(
            FILE,
            format!("run: {{ id: r1, agent: '{lead}' }}\nnodes: [{{ id: a, status: failed }}, {{ id: b }}]"),
        );
        let desc = descriptor("bind: {}\n");
        let mut run = new_run();
        for (state, want) in [
            (AgentState::Busy, NodeStatus::Busy),
            (AgentState::Waiting, NodeStatus::Busy),
            (AgentState::Idle, NodeStatus::Failed),
            (AgentState::Gone, NodeStatus::Failed),
        ] {
            agents.state(lead, state);
            let s = run.derive(&desc, &files, &agents);
            assert_eq!(s.header.status, want, "with the lead {state:?}");
            assert_eq!(run.summary().unwrap().done, want == NodeStatus::Failed);
        }
    }

    #[test]
    fn a_waiting_lead_needs_you_until_the_run_ends() {
        let mut files = MemFiles::default();
        let mut agents = FakeAgents::default();
        let lead = agents.add("lead", None);
        agents.state(lead, AgentState::Waiting);
        let desc = descriptor("bind: {}\n");
        let mut run = new_run();
        let file = |status: &str| {
            format!("run: {{ id: r1, agent: '{lead}' }}\nnodes: [{{ id: a, status: {status} }}]")
        };
        files.put(FILE, file("blocked"));
        run.derive(&desc, &files, &agents);
        assert!(run.summary().unwrap().needs_you);
        files.put(FILE, file("done"));
        run.derive(&desc, &files, &agents);
        assert!(!run.summary().unwrap().needs_you);
        agents.state(lead, AgentState::Busy);
        files.put(FILE, file("waiting_human"));
        run.derive(&desc, &files, &agents);
        assert!(run.summary().unwrap().needs_you);
    }

    #[test]
    fn a_standard_word_reads_as_itself_beside_a_status_map() {
        let mut files = MemFiles::default();
        files.put(
            FILE,
            "nodes: [{ id: a, status: in_progress }, { id: b, status: done }, { id: c, status: nope }]",
        );
        let desc = descriptor("nodes: { status_map: { in_progress: busy } }\n");
        let s = new_run().derive(&desc, &files, &crate::inputs::NoAgents);
        assert_eq!(node(&s, "a").status, NodeStatus::Busy);
        assert_eq!(node(&s, "b").status, NodeStatus::Done);
        assert_eq!(node(&s, "c").status, NodeStatus::Unknown);
    }

    #[test]
    fn with_no_log_the_engine_keeps_its_own_history() {
        let mut files = MemFiles::default();
        files.put(FILE, "nodes: [{ id: a, status: busy }, { id: b }]");
        let desc = descriptor("");
        let mut agents = FakeAgents::default();
        agents.now = Timestamp(1_000);
        let mut run = new_run();
        run.derive(&desc, &files, &agents);
        agents.now = Timestamp(2_000);
        files.put(
            FILE,
            "nodes: [{ id: a, status: done }, { id: b, status: busy }]",
        );
        let s = run.derive(&desc, &files, &agents);
        assert_eq!(run.events().len(), 3);
        let a = &node(&s, "a").attempts;
        assert_eq!(
            (a[0].start, a[0].end, a[0].end_status),
            (
                Timestamp(1_000),
                Some(Timestamp(2_000)),
                Some(NodeStatus::Done)
            )
        );
    }

    #[test]
    fn a_run_file_that_stops_parsing_keeps_the_last_good_state() {
        let mut files = MemFiles::default();
        files.put(FILE, REFERENCE);
        let desc = descriptor("");
        let mut run = new_run();
        let good = run.derive(&desc, &files, &FakeAgents::default());
        files.put(FILE, "");
        let reads = files.reads();
        let s = run.derive(&desc, &files, &FakeAgents::default());
        assert_eq!(files.reads(), reads + 1 + RETRIES as usize);
        assert_eq!(s.nodes, good.nodes);
        assert_eq!(s.notice.as_ref().map(|n| n.tone), Some(Tone::Warn));
        assert_eq!(s.rev, good.rev + 1, "the notice is a change");
    }

    #[test]
    fn a_newer_minor_says_so() {
        let mut files = MemFiles::default();
        files.put(FILE, REFERENCE);
        let text = "format: maxhq.workflow/1.4\nid: example\nruns: { glob: .runs/*/run.yaml, someday: 1 }\n";
        let desc = load(text.as_bytes()).unwrap();
        let s = new_run().derive(&desc, &files, &FakeAgents::default());
        assert_eq!(s.notice.unwrap().tone, Tone::Info);
    }

    #[test]
    fn rev_counts_only_changes_and_the_hash_ignores_status() {
        let mut files = MemFiles::default();
        files.put(FILE, REFERENCE);
        let desc = descriptor("");
        let mut run = new_run();
        let one = run.derive(&desc, &files, &FakeAgents::default());
        let again = run.derive(&desc, &files, &FakeAgents::default());
        assert_eq!(again.rev, one.rev);
        files.put(FILE, REFERENCE.replace("status: busy", "status: done"));
        let two = run.derive(&desc, &files, &FakeAgents::default());
        assert_eq!(two.rev, one.rev + 1);
        assert_eq!(two.shape.hash, one.shape.hash);
        assert_eq!(one.shape.hash.len(), 16);
        files.put(FILE, REFERENCE.replace("needs: [plan], ", ""));
        let three = run.derive(&desc, &files, &FakeAgents::default());
        assert_ne!(three.shape.hash, one.shape.hash);
    }

    #[test]
    fn maxhq_agents_draws_a_lead_and_its_sub_agents() {
        let mut agents = FakeAgents::default();
        let lead = agents.add("Fix the login page", None);
        let a = agents.add("Explore the form", Some(lead));
        let b = agents.add("Write tests", Some(lead));
        let c = agents.add("Ask about scope", Some(lead));
        agents.state(a, AgentState::Gone);
        agents.state(c, AgentState::Waiting);
        agents.add("someone else", None);
        let desc = Descriptor::agents();
        let mut run = Run::lead(
            DeviceId::derive("d"),
            ProjectId::derive("p"),
            Path::new(PROJECT),
            lead,
        );
        let s = run.derive(&desc, &MemFiles::default(), &agents);
        assert_eq!(s.key.workflow, "maxhq.agents");
        assert_eq!(s.header.label, "Fix the login page");
        let labels: Vec<_> = s.shape.nodes.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Explore the form", "Write tests", "Ask about scope"]
        );
        assert_eq!(s.nodes[0].status, NodeStatus::Done);
        assert_eq!(s.nodes[1].status, NodeStatus::Busy);
        assert_eq!(s.nodes[1].agents, vec![b]);
        assert_eq!(s.nodes[2].status, NodeStatus::WaitingHuman);
        assert_eq!(run.orchestrators(), [lead]);
        let summary = run.summary().unwrap();
        assert!(summary.needs_you);
        assert_eq!(summary.current, vec![b.to_string()]);
    }

    #[test]
    fn maxhq_agents_keeps_every_sub_agent_reached_so_far() {
        let mut agents = FakeAgents::default();
        let lead = agents.add("Fix the login page", None);
        let a = agents.add("Explore the form", Some(lead));
        let desc = Descriptor::agents();
        let mut run = Run::lead(
            DeviceId::derive("d"),
            ProjectId::derive("p"),
            Path::new(PROJECT),
            lead,
        );
        let first = run.derive(&desc, &MemFiles::default(), &agents);
        assert_eq!(first.nodes.len(), 1);
        // Claude lists a sub-agent only while it runs: the next one starts
        // and the first one leaves the list.
        let b = agents.add("Write tests", Some(lead));
        agents.remove(a);
        let s = run.derive(&desc, &MemFiles::default(), &agents);
        let labels: Vec<_> = s.shape.nodes.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(labels, ["Explore the form", "Write tests"]);
        assert_ne!(s.shape.hash, first.shape.hash);
        assert_eq!(s.nodes[0].status, NodeStatus::Done);
        assert_eq!(s.nodes[0].agents, vec![a]);
        assert_eq!(s.nodes[1].status, NodeStatus::Busy);
        assert_eq!(s.nodes[1].agents, vec![b]);
        let summary = run.summary().unwrap();
        assert_eq!(summary.progress.done, 1);
        assert_eq!(summary.agents, vec![a, b]);
        assert!(!summary.failed);
    }

    #[test]
    fn host_nodes_take_the_lead_and_header_lines_fill_in() {
        let mut files = MemFiles::default();
        let mut agents = FakeAgents::default();
        let lead = agents.add("lead", None);
        files.put(FILE, format!("run: {{ id: r1, agent: '{lead}', rounds: 2 }}\nnodes: [{{ id: plan, status: busy }}]"));
        let desc = descriptor(
            "  facts: { rounds: { path: $.run.rounds } }
nodes: { info: { plan: { host: true } } }
header:
  - { text: 'round {run.rounds}', tone: warn, when: { fact: { rounds: { gte: 2 } } } }
  - { text: 'never', when: { fact: missing } }
bind: {}
",
        );
        let mut run = new_run();
        files.put(
            FILE,
            format!("run: {{ id: r1, agent: '{lead}', rounds: 2 }}\nnodes: [{{ id: plan }}]"),
        );
        let s = run.derive(&desc, &files, &agents);
        assert!(
            node(&s, "plan").agents.is_empty(),
            "not while it waits its turn"
        );
        files.put(FILE, format!("run: {{ id: r1, agent: '{lead}', rounds: 2 }}\nnodes: [{{ id: plan, status: busy }}]"));
        let s = run.derive(&desc, &files, &agents);
        assert_eq!(node(&s, "plan").agents, vec![lead]);
        assert_eq!(
            run.header_lines(),
            [Badge {
                text: "round 2".into(),
                tone: Tone::Warn
            }]
        );
    }

    #[test]
    fn detail_lists_keep_the_last_items() {
        let mut files = MemFiles::default();
        files.put(
            FILE,
            "nodes: [{ id: a, status: done, touched: [x.rs, y.rs, z.rs] }]",
        );
        let desc = descriptor(
            "nodes: { fields: [touched] }
detail:
  - { label: Files, list: { path: '$.nodes[0].touched' }, limit: 2 }
  - { label: None, list: { path: $.missing } }
",
        );
        let mut run = new_run();
        run.derive(&desc, &files, &FakeAgents::default());
        let detail = run.detail(&desc, &files, "a");
        assert_eq!(detail.lists[0].items, ["y.rs", "z.rs"]);
        assert!(detail.lists[1].items.is_empty());
        assert!(run.detail(&desc, &files, "nope").lists.is_empty());
    }
}
