// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The descriptor's small language: templates, value sources, matchers and
//! predicates, evaluated against one run and, usually, one of its nodes.

use std::borrow::Cow;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use maxhq_protocol::workflow::NodeStatus;
use regex::Regex;
use serde_json::{Map, Value as Json};

use crate::descriptor::{
    Compare, Each, Expr, FactTest, GroupRef, Matcher, NodeInfo, Nodes, Operand, Pick, Predicate,
    Source,
};
use crate::inputs::{AgentState, Files};
use crate::load::MAX_FILE;

/// The most tests one derive may run. `any_node` tries every row, so a few
/// nested ones multiply past anything the follower's one thread can finish.
const MAX_TESTS: u64 = 5_000_000;

/// Where a run's files are, and what this derive has read of them. Every
/// file is read at most once per derive.
pub(crate) struct Scope<'a> {
    pub files: &'a dyn Files,
    pub project: &'a Path,
    /// The folder holding the workflow's `.maxhq/`.
    pub workflow: &'a Path,
    /// The run's folder, which relative paths start from.
    pub run_dir: &'a Path,
    /// The run file, for a `match` with no `in`.
    pub run_file: Option<&'a Path>,
    /// The run file, parsed.
    pub doc: &'a Json,
    docs: RefCell<HashMap<PathBuf, Option<Rc<Json>>>>,
    texts: RefCell<HashMap<PathBuf, Option<Rc<str>>>>,
    patterns: RefCell<HashMap<String, Option<Rc<Regex>>>>,
    tests: Cell<u64>,
}

impl<'a> Scope<'a> {
    pub fn new(
        files: &'a dyn Files,
        project: &'a Path,
        workflow: &'a Path,
        run_dir: &'a Path,
        run_file: Option<&'a Path>,
        doc: &'a Json,
    ) -> Self {
        Self {
            files,
            project,
            workflow,
            run_dir,
            run_file,
            doc,
            docs: RefCell::default(),
            texts: RefCell::default(),
            patterns: RefCell::default(),
            tests: Cell::new(0),
        }
    }

    /// Counts one test, and says whether this derive may still run it.
    fn test(&self) -> bool {
        self.tests.set(self.tests.get().saturating_add(1));
        !self.gave_up()
    }

    /// Whether this derive ran out of tests, so what it says may be wrong.
    pub fn gave_up(&self) -> bool {
        self.tests.get() > MAX_TESTS
    }

    /// Resolves a path template from the run folder, or `None` when it
    /// reads nothing or would leave both the project and the workflow
    /// folder.
    pub fn resolve(&self, template: &str, env: &Env) -> Option<PathBuf> {
        let rendered = render(template, &|name| env.lookup(name), &|s| Cow::Borrowed(s))?;
        let path = self.run_dir.join(rendered);
        let path = normalize(&path)?;
        (path.starts_with(self.project) || path.starts_with(self.workflow)).then_some(path)
    }

    /// A file's text, capped at [`MAX_FILE`].
    pub fn text(&self, path: &Path) -> Option<Rc<str>> {
        if let Some(text) = self.texts.borrow().get(path) {
            return text.clone();
        }
        let text = read_capped(self.files, path)
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(Rc::from);
        self.texts
            .borrow_mut()
            .insert(path.to_owned(), text.clone());
        text
    }

    /// A YAML or JSON file, parsed.
    fn doc(&self, path: &Path) -> Option<Rc<Json>> {
        if let Some(doc) = self.docs.borrow().get(path) {
            return doc.clone();
        }
        let doc = self
            .text(path)
            .and_then(|text| parse_doc(text.as_bytes()))
            .map(Rc::new);
        self.docs.borrow_mut().insert(path.to_owned(), doc.clone());
        doc
    }

    fn pattern(&self, text: String) -> Option<Rc<Regex>> {
        if let Some(pattern) = self.patterns.borrow().get(&text) {
            return pattern.clone();
        }
        let pattern = Regex::new(&text).ok().map(Rc::new);
        self.patterns.borrow_mut().insert(text, pattern.clone());
        pattern
    }
}

/// Reads a whole file if it's no larger than [`MAX_FILE`].
pub(crate) fn read_capped(files: &dyn Files, path: &Path) -> Option<Vec<u8>> {
    let stat = files.stat(path).ok()?;
    if stat.size > MAX_FILE {
        return None;
    }
    // The file can have grown since the stat.
    files
        .read(path)
        .ok()
        .filter(|bytes| bytes.len() as u64 <= MAX_FILE)
}

/// Parses YAML or JSON into a JSON value. Empty files read as nothing.
pub(crate) fn parse_doc(bytes: &[u8]) -> Option<Json> {
    crate::load::check_yaml(bytes).ok()?;
    serde_norway::from_slice::<Json>(bytes)
        .ok()
        .filter(|doc| !doc.is_null())
}

/// Removes `.` and `..` from a path without touching the disk. `None` when
/// `..` would climb above the root.
pub(crate) fn normalize(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            other => out.push(other),
        }
    }
    Some(out)
}

/// Fills `{name}` placeholders through `lookup`, passing each value through
/// `escape`. Braces around anything that isn't a name stay as they are, so
/// globs and regex counts survive. `None` when a name has no value.
pub(crate) fn render(
    template: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    escape: &dyn Fn(&str) -> Cow<str>,
) -> Option<String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}').map(|close| (&after[..close], close)) {
            Some((name, close)) if is_name(name) => {
                out.push_str(&escape(&lookup(name)?));
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Some(out)
}

fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
}

/// A value as template text. Nothing for null and maps.
pub(crate) fn text(value: &Json) -> Option<String> {
    match value {
        Json::Null | Json::Object(_) => None,
        Json::String(s) => Some(s.clone()),
        Json::Bool(b) => Some(b.to_string()),
        Json::Number(n) => Some(n.to_string()),
        Json::Array(items) => Some(items.iter().filter_map(text).collect::<Vec<_>>().join(", ")),
    }
}

/// A number as JSON, whole when it is.
fn number(n: f64) -> Json {
    if n.fract() == 0.0 && n.abs() < 9.0e15 {
        Json::from(n as i64)
    } else {
        serde_json::Number::from_f64(n).map_or(Json::Null, Json::Number)
    }
}

/// What the run says about itself, for `{run.<name>}` and `fact`.
#[derive(Clone, Debug, Default)]
pub(crate) struct RunFacts {
    pub id: String,
    pub label: String,
    pub facts: Vec<(String, Json)>,
}

impl RunFacts {
    pub fn get(&self, name: &str) -> Option<Json> {
        match name {
            "id" => Some(Json::from(self.id.as_str())),
            "label" => Some(Json::from(self.label.as_str())),
            _ => self
                .facts
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone()),
        }
    }
}

/// One node as rules see it: its id, its status so far and its fields.
#[derive(Clone, Debug)]
pub(crate) struct NodeRow {
    pub id: String,
    pub status: NodeStatus,
    /// The workflow's own status word.
    pub word: Option<String>,
    pub fields: Map<String, Json>,
    pub group: Option<String>,
    pub needs: Vec<String>,
    pub info: Option<NodeInfo>,
}

impl NodeRow {
    /// A field by its standard name or one of the descriptor's `fields`.
    pub fn field(&self, nodes: &Nodes, name: &str) -> Option<Json> {
        let key = match name {
            "id" => return Some(Json::from(self.id.as_str())),
            "status" => return self.word.as_deref().map(Json::from),
            "group" => return self.group.as_deref().map(Json::from),
            "attempts" => &nodes.attempts,
            "label" => &nodes.label,
            "agent" => &nodes.agent,
            other if nodes.fields.iter().any(|f| f == other) => other,
            _ => return None,
        };
        self.fields.get(key).filter(|v| !v.is_null()).cloned()
    }
}

/// Everything an expression or predicate can see.
#[derive(Clone, Copy)]
pub(crate) struct Env<'a> {
    pub scope: &'a Scope<'a>,
    pub nodes_map: &'a Nodes,
    pub run: &'a RunFacts,
    pub rows: &'a [NodeRow],
    pub orchestrator: AgentState,
    /// The node under test.
    pub node: Option<&'a NodeRow>,
    /// Extra variables, such as a badge's `vars`.
    pub vars: &'a [(String, Json)],
    /// The value under test, read on first use.
    pub value: Option<(&'a Expr, &'a OnceCell<Option<Json>>)>,
}

impl<'a> Env<'a> {
    /// The same run with another node under test and no value or vars.
    pub fn with_node(self, node: Option<&'a NodeRow>) -> Self {
        Self {
            node,
            vars: &[],
            value: None,
            ..self
        }
    }

    /// The value under test.
    pub fn value(&self) -> Option<Json> {
        let (expr, cell) = self.value?;
        let inner = Self {
            value: None,
            ..*self
        };
        cell.get_or_init(|| eval(expr, &inner)).clone()
    }

    /// A template variable.
    pub fn lookup(&self, name: &str) -> Option<String> {
        let value = match name {
            "value" => self.value(),
            "project" => return Some(self.scope.project.to_string_lossy().into_owned()),
            "workflow" => return Some(self.scope.workflow.to_string_lossy().into_owned()),
            _ => {
                if let Some(fact) = name.strip_prefix("run.") {
                    self.run.get(fact)
                } else if let Some(var) = name.strip_prefix("vars.") {
                    self.var(var)
                } else {
                    self.node.and_then(|n| n.field(self.nodes_map, name))
                }
            }
        };
        value.as_ref().and_then(text)
    }

    fn var(&self, name: &str) -> Option<Json> {
        if let Some((_, v)) = self.vars.iter().find(|(k, _)| k == name) {
            return Some(v.clone());
        }
        let info = self.node?.info.as_ref()?;
        info.vars.get(name).map(|v| Json::from(v.as_str()))
    }
}

/// Evaluates a value. Null reads as nothing.
pub(crate) fn eval(expr: &Expr, env: &Env) -> Option<Json> {
    if !env.scope.test() {
        return None;
    }
    match expr {
        Expr::Literal(Json::Object(_)) => None,
        Expr::Literal(value) => Some(value.clone()).filter(|v| !v.is_null()),
        Expr::Source(source) => source_value(source, env).or_else(|| source.default.clone()),
    }
}

fn source_value(source: &Source, env: &Env) -> Option<Json> {
    let scope = env.scope;
    let file = match &source.file {
        Some(template) => Some(scope.resolve(template, env)?),
        None => None,
    };
    if let Some(pid_file) = &source.alive_pid {
        let path = scope.resolve(pid_file, env)?;
        let alive = scope
            .text(&path)
            .and_then(|text| text.trim().parse::<u32>().ok())
            .is_some_and(|pid| scope.files.alive(pid));
        return Some(Json::Bool(alive));
    }
    if let Some(pattern) = &source.pattern {
        let path = file.as_deref().or(scope.run_file)?;
        let text = scope.text(path)?;
        let pattern = render(pattern, &|name| env.lookup(name), &|s| {
            regex::escape(s).into()
        })?;
        let regex = scope.pattern(pattern)?;
        return Some(pick(
            &regex,
            &text,
            source.pick.unwrap_or(Pick::First),
            source.group.as_ref(),
        ))
        .filter(|v| !v.is_null());
    }
    let owned;
    let doc: &Json = match &file {
        Some(path) => {
            owned = scope.doc(path)?;
            &owned
        }
        None => scope.doc,
    };
    if let Some(query) = &source.path {
        let found: Vec<&Json> = query
            .0
            .query(doc)
            .all()
            .into_iter()
            .filter(|v| !v.is_null())
            .collect();
        return match found.as_slice() {
            [] => None,
            [one] => Some((*one).clone()),
            many => Some(Json::Array(many.iter().map(|v| (*v).clone()).collect())),
        };
    }
    if let Some(query) = &source.count {
        let found = query.0.query(doc).all();
        let count = match found.as_slice() {
            [Json::Array(items)] => items.len(),
            other => other.len(),
        };
        return Some(Json::from(count));
    }
    if let Some(query) = &source.sum {
        let each = source.each.unwrap_or_default();
        let total = query
            .0
            .query(doc)
            .all()
            .into_iter()
            .filter_map(Json::as_f64)
            .map(|n| adjust(n, each))
            .sum();
        return Some(number(total));
    }
    None
}

fn adjust(n: f64, each: Each) -> f64 {
    let n = n + each.add.unwrap_or(0.0);
    let n = each.min.map_or(n, |min| n.max(min));
    each.max.map_or(n, |max| n.min(max))
}

fn pick(regex: &Regex, text: &str, pick: Pick, group: Option<&GroupRef>) -> Json {
    let take = |line: &str| -> Option<Json> {
        let caps = regex.captures(line)?;
        let m = match group {
            None => caps.get(0),
            Some(GroupRef::Index(i)) => caps.get(*i),
            Some(GroupRef::Name(name)) => caps.name(name),
        };
        Some(Json::from(m.map_or("", |m| m.as_str())))
    };
    let mut lines = text.lines();
    match pick {
        Pick::First => lines.find_map(take).unwrap_or(Json::Null),
        Pick::Last => lines.rev().find_map(take).unwrap_or(Json::Null),
        Pick::Count => Json::from(lines.filter(|l| regex.is_match(l)).count()),
        Pick::All => Json::Array(lines.filter_map(take).collect()),
    }
}

/// Equality that reads `1` and `1.0` as one number, and a string the same
/// as a number or flag with the same text.
fn same(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Number(x), Json::Number(y)) => x.as_f64() == y.as_f64(),
        (Json::String(_), Json::Number(_) | Json::Bool(_))
        | (Json::Number(_) | Json::Bool(_), Json::String(_)) => text(a) == text(b),
        _ => a == b,
    }
}

fn as_number(value: &Json) -> Option<f64> {
    match value {
        Json::Number(n) => n.as_f64(),
        Json::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn operand(op: &Operand, env: &Env) -> Option<Json> {
    match op {
        Operand::Fact { fact } => env.run.get(fact),
        Operand::Literal(value) => Some(value.clone()),
    }
}

/// Whether a value passes a matcher. A missing value passes only
/// `exists: false` and `ne`.
pub(crate) fn matches(matcher: &Matcher, value: Option<&Json>, env: &Env) -> bool {
    if !env.scope.test() {
        return false;
    }
    match matcher {
        Matcher::Is(want) => value.is_some_and(|v| same(v, want)),
        Matcher::OneOf(list) => value.is_some_and(|v| list.iter().any(|want| same(v, want))),
        Matcher::Compare(compare) => compares(compare, value, env),
    }
}

fn compares(c: &Compare, value: Option<&Json>, env: &Env) -> bool {
    if c.exists.is_some_and(|want| want != value.is_some()) {
        return false;
    }
    let side = |op: &Option<Operand>| op.as_ref().map(|op| operand(op, env));
    let equal = |want: &Option<Json>| matches!((value, want), (Some(v), Some(w)) if same(v, w));
    if side(&c.eq).is_some_and(|want| !equal(&want)) || side(&c.ne).is_some_and(|want| equal(&want))
    {
        return false;
    }
    let ordered = [(&c.gt, 1), (&c.gte, 2), (&c.lt, 3), (&c.lte, 4)];
    ordered.into_iter().all(|(op, kind)| {
        let Some(want) = side(op) else { return true };
        let (Some(v), Some(w)) = (value.and_then(as_number), want.as_ref().and_then(as_number))
        else {
            return false;
        };
        match kind {
            1 => v > w,
            2 => v >= w,
            3 => v < w,
            _ => v <= w,
        }
    })
}

/// Whether a predicate holds for the node under test.
pub(crate) fn holds(p: &Predicate, env: &Env) -> bool {
    if !env.scope.test() {
        return false;
    }
    let node = env.node;
    let facts = match &p.fact {
        Some(FactTest::Name(name)) => env.run.get(name).is_some() == p.exists.unwrap_or(true),
        Some(FactTest::Each(facts)) => facts
            .iter()
            .all(|(name, m)| matches(m, env.run.get(name).as_ref(), env)),
        None => true,
    };
    let held = p
        .node
        .as_ref()
        .is_none_or(|ids| node.is_some_and(|n| ids.contains(&n.id)))
        && p.status
            .as_ref()
            .is_none_or(|statuses| node.is_some_and(|n| statuses.contains(&n.status)))
        && p.field.as_ref().is_none_or(|fields| {
            node.is_some_and(|n| {
                fields
                    .iter()
                    .all(|(name, m)| matches(m, n.field(env.nodes_map, name).as_ref(), env))
            })
        })
        && facts
        && p.orchestrator
            .as_ref()
            .is_none_or(|states| states.contains(&env.orchestrator))
        && p.any_node.as_ref().is_none_or(|inner| {
            env.rows
                .iter()
                .any(|row| holds(inner, &env.with_node(Some(row))))
        })
        && p.all
            .as_ref()
            .is_none_or(|all| all.iter().all(|q| holds(q, env)))
        && p.any
            .as_ref()
            .is_none_or(|any| any.iter().any(|q| holds(q, env)))
        && p.not.as_ref().is_none_or(|not| !holds(not, env));
    // Last, since it may read a file.
    held && p
        .value
        .as_ref()
        .is_none_or(|m| matches(m, env.value().as_ref(), env))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::Descriptor;
    use crate::fake::MemFiles;

    const PROJECT: &str = "/work/example-app";

    fn row(id: &str, status: NodeStatus, fields: Json) -> NodeRow {
        NodeRow {
            id: id.into(),
            status,
            word: None,
            fields: fields.as_object().cloned().unwrap_or_default(),
            group: None,
            needs: Vec::new(),
            info: None,
        }
    }

    struct Fixture {
        files: MemFiles,
        doc: Json,
        nodes: Nodes,
        run: RunFacts,
        rows: Vec<NodeRow>,
    }

    impl Fixture {
        fn new() -> Self {
            let nodes = Nodes {
                fields: vec!["tier".into(), "effort".into()],
                ..Nodes::default()
            };
            Self {
                files: MemFiles::default(),
                doc: serde_json::json!({
                    "run": { "id": "r1" },
                    "graph": [{ "attempts": 3 }, { "attempts": 1 }, { "attempts": 0 }],
                    "findings": [
                        { "status": "open", "severity": "major" },
                        { "status": "open", "severity": "minor" },
                        { "status": "fixed", "severity": "major" },
                    ],
                    "rounds": [1, 2, 3],
                    "nothing": null,
                }),
                nodes,
                run: RunFacts {
                    id: "r1".into(),
                    label: "Example".into(),
                    facts: vec![
                        ("ceiling".into(), Json::from(12)),
                        ("preset".into(), Json::from("fast")),
                    ],
                },
                rows: vec![
                    row("plan", NodeStatus::Done, serde_json::json!({ "tier": "M" })),
                    row(
                        "build",
                        NodeStatus::Busy,
                        serde_json::json!({ "effort": "off", "attempts": 2 }),
                    ),
                ],
            }
        }

        fn with<T>(&self, node: Option<usize>, f: impl FnOnce(&Env) -> T) -> T {
            let project = Path::new(PROJECT);
            let run_dir = project.join("runs/r1");
            let run_file = run_dir.join("run.yaml");
            let scope = Scope::new(
                &self.files,
                project,
                project,
                &run_dir,
                Some(&run_file),
                &self.doc,
            );
            let env = Env {
                scope: &scope,
                nodes_map: &self.nodes,
                run: &self.run,
                rows: &self.rows,
                orchestrator: AgentState::Idle,
                node: node.map(|i| &self.rows[i]),
                vars: &[],
                value: None,
            };
            f(&env)
        }

        fn value(&self, yaml: &str) -> Option<Json> {
            let expr: Expr = serde_norway::from_str(yaml).unwrap();
            self.with(Some(1), |env| eval(&expr, env))
        }

        fn holds(&self, node: usize, yaml: &str) -> bool {
            let p: Predicate = serde_norway::from_str(yaml).unwrap();
            self.with(Some(node), |env| holds(&p, env))
        }
    }

    #[test]
    fn templates_fill_names_and_keep_other_braces() {
        let lookup = |name: &str| (name == "a").then(|| "x".to_string());
        fn plain(s: &str) -> Cow<'_, str> {
            Cow::Borrowed(s)
        }
        assert_eq!(
            render("{a}/{b,c}/\\d{2}/{ a }", &lookup, &plain).unwrap(),
            "x/{b,c}/\\d{2}/{ a }"
        );
        assert_eq!(render("{missing}", &lookup, &plain), None);
        assert_eq!(render("{a", &lookup, &plain).unwrap(), "{a");
    }

    #[test]
    fn templates_read_fields_facts_vars_and_values() {
        let f = Fixture::new();
        f.with(Some(1), |env| {
            assert_eq!(env.lookup("id").unwrap(), "build");
            assert_eq!(env.lookup("attempts").unwrap(), "2");
            assert_eq!(env.lookup("effort").unwrap(), "off");
            assert_eq!(env.lookup("run.preset").unwrap(), "fast");
            assert_eq!(env.lookup("run.id").unwrap(), "r1");
            assert_eq!(env.lookup("project").unwrap(), PROJECT);
            assert_eq!(env.lookup("tier"), None);
            assert_eq!(env.lookup("unlisted"), None);
        });
    }

    #[test]
    fn path_reads_one_value_or_a_list() {
        let f = Fixture::new();
        assert_eq!(f.value("{ path: $.run.id }").unwrap(), "r1");
        assert_eq!(
            f.value("{ path: \"$.graph[*].attempts\" }").unwrap(),
            serde_json::json!([3, 1, 0])
        );
        assert_eq!(f.value("{ path: $.missing }"), None);
        assert_eq!(f.value("{ path: $.nothing, default: 5 }").unwrap(), 5);
    }

    #[test]
    fn count_counts_matches_or_one_list() {
        let f = Fixture::new();
        let open = "{ count: \"$.findings[?@.status == 'open' && (@.severity == 'blocker' || @.severity == 'major')]\" }";
        assert_eq!(f.value(open).unwrap(), 1);
        assert_eq!(f.value("{ count: $.rounds }").unwrap(), 3);
        assert_eq!(f.value("{ count: $.missing }").unwrap(), 0);
    }

    #[test]
    fn sum_adjusts_each_number() {
        let f = Fixture::new();
        assert_eq!(f.value("{ sum: \"$.graph[*].attempts\" }").unwrap(), 4);
        assert_eq!(
            f.value("{ sum: \"$.graph[*].attempts\", each: { add: -1, min: 0 } }")
                .unwrap(),
            2
        );
        assert_eq!(
            f.value("{ sum: \"$.graph[*].attempts\", each: { max: 1 } }")
                .unwrap(),
            2
        );
    }

    #[test]
    fn match_picks_lines_and_groups() {
        let mut f = Fixture::new();
        f.files.put(
            format!("{PROJECT}/runs/r1/log.md"),
            "- 1 build: busy.\n- 2 plan: done.\n- 3 build: done.\nport 4100\n",
        );
        let m =
            |rest: &str| format!("{{ match: '^- (\\d+) {{id}}: (?<s>\\w+)', in: log.md{rest} }}");
        assert_eq!(f.value(&m("")).unwrap(), "- 1 build: busy");
        assert_eq!(f.value(&m(", pick: last, group: 1")).unwrap(), "3");
        assert_eq!(f.value(&m(", pick: last, group: s")).unwrap(), "done");
        assert_eq!(f.value(&m(", pick: count")).unwrap(), 2);
        assert_eq!(
            f.value(&m(", pick: all, group: s")).unwrap(),
            serde_json::json!(["busy", "done"])
        );
        assert_eq!(f.value("{ match: '^nope', in: log.md }"), None);
        assert_eq!(
            f.value("{ match: '^nope', in: log.md, pick: count }")
                .unwrap(),
            0
        );
        assert_eq!(f.value("{ match: 'x', in: missing.md, pick: count }"), None);
    }

    #[test]
    fn match_escapes_what_it_fills_in() {
        let mut f = Fixture::new();
        f.rows[1].id = "a.b".into();
        f.files
            .put(format!("{PROJECT}/runs/r1/log.md"), "axb\na.b\n");
        assert_eq!(
            f.value("{ match: '^{id}$', in: log.md, pick: count }")
                .unwrap(),
            1
        );
    }

    #[test]
    fn alive_pid_reads_a_pid_file() {
        let mut f = Fixture::new();
        assert_eq!(f.value("{ alive_pid: server.pid }").unwrap(), false);
        f.files
            .put(format!("{PROJECT}/runs/r1/server.pid"), "4242\n");
        assert_eq!(f.value("{ alive_pid: server.pid }").unwrap(), false);
        f.files.set_alive(4242, true);
        assert_eq!(f.value("{ alive_pid: server.pid }").unwrap(), true);
    }

    #[test]
    fn in_reads_other_files_from_the_run_folder_or_the_project() {
        let mut f = Fixture::new();
        f.files
            .put(format!("{PROJECT}/runs/r1/build.yaml"), "state: awaiting\n");
        f.files.put(format!("{PROJECT}/config.yaml"), "limit: 7\n");
        assert_eq!(
            f.value("{ path: $.state, in: \"{id}.yaml\" }").unwrap(),
            "awaiting"
        );
        assert_eq!(
            f.value("{ path: $.limit, in: \"{project}/config.yaml\" }")
                .unwrap(),
            7
        );
    }

    #[test]
    fn an_in_path_that_leaves_the_project_is_refused() {
        let mut f = Fixture::new();
        f.files.put("/work/secret.yaml", "a: 1\n");
        f.files.put("/etc/hosts.yaml", "a: 1\n");
        let before = f.files.reads();
        assert_eq!(f.value("{ path: $.a, in: ../../../secret.yaml }"), None);
        assert_eq!(f.value("{ path: $.a, in: /etc/hosts.yaml }"), None);
        assert_eq!(
            f.value("{ path: $.a, in: \"{project}/../secret.yaml\" }"),
            None
        );
        assert_eq!(f.files.reads(), before);
    }

    #[test]
    fn predicates_test_status_node_and_fields() {
        let f = Fixture::new();
        assert!(f.holds(1, "{ status: busy }"));
        assert!(f.holds(1, "{ status: [done, busy], node: build }"));
        assert!(!f.holds(1, "{ node: [plan] }"));
        assert!(f.holds(1, "{ field: { effort: \"off\" } }"));
        assert!(f.holds(1, "{ field: { effort: [light, \"off\"] } }"));
        assert!(f.holds(1, "{ field: { attempts: { gt: 1, lte: 2 } } }"));
        assert!(!f.holds(1, "{ field: { attempts: { gte: 3 } } }"));
        assert!(f.holds(0, "{ field: { tier: { exists: true } } }"));
        assert!(f.holds(1, "{ field: { tier: { exists: false } } }"));
        assert!(f.holds(1, "{ field: { attempts: \"2\" } }"));
    }

    #[test]
    fn predicates_test_facts() {
        let f = Fixture::new();
        assert!(f.holds(1, "{ fact: { ceiling: { gte: 12 } } }"));
        assert!(!f.holds(
            1,
            "{ fact: { ceiling: { lt: { fact: missing } } }, not: { status: done } }"
        ));
        assert!(f.holds(1, "{ fact: { ceiling: { eq: { fact: ceiling } } } }"));
        assert!(f.holds(1, "{ fact: preset }"));
        assert!(f.holds(1, "{ fact: missing, exists: false }"));
        assert!(!f.holds(1, "{ fact: missing, exists: true }"));
    }

    #[test]
    fn predicates_combine_and_look_at_other_nodes() {
        let f = Fixture::new();
        assert!(f.holds(1, "{ any_node: { id: plan, status: done } }"));
        assert!(!f.holds(1, "{ any_node: { id: plan, status: busy } }"));
        assert!(f.holds(1, "{ any: [{ status: done }, { status: busy }] }"));
        assert!(!f.holds(1, "{ all: [{ status: done }, { status: busy }] }"));
        assert!(f.holds(1, "{ not: { status: done } }"));
        assert!(f.holds(1, "{ orchestrator: [idle, gone] }"));
        assert!(!f.holds(1, "{ orchestrator: waiting }"));
        assert!(f.holds(1, "{}"));
    }

    #[test]
    fn the_value_predicate_reads_the_value_once() {
        let mut f = Fixture::new();
        f.files.put(
            format!("{PROJECT}/runs/r1/s.yaml"),
            "state: limit_reached\n",
        );
        let expr: Expr = serde_norway::from_str("{ path: $.state, in: s.yaml }").unwrap();
        let p: Predicate = serde_norway::from_str("{ value: limit_reached }").unwrap();
        let skipped: Predicate =
            serde_norway::from_str("{ status: done, value: limit_reached }").unwrap();
        let cell = OnceCell::new();
        let before = f.files.reads();
        f.with(Some(1), |env| {
            let env = Env {
                value: Some((&expr, &cell)),
                ..*env
            };
            assert!(!holds(&skipped, &env));
            assert_eq!(f.files.reads(), before);
            assert!(holds(&p, &env));
            assert!(holds(&p, &env));
            assert_eq!(env.lookup("value").unwrap(), "limit_reached");
        });
        assert_eq!(f.files.reads(), before + 1);
    }

    #[test]
    fn descriptor_types_round_trip_the_devflow_shapes() {
        // The unusual shapes the descriptor language allows, in one place.
        let d: Descriptor = serde_norway::from_str(
            "format: maxhq.workflow/1.0\nid: x\nruns: { glob: a }\nrules:\n  - when: { fact: reclassed, exists: false }\n    provisional: true\n",
        )
        .unwrap();
        assert!(matches!(d.rules[0].when.fact, Some(FactTest::Name(_))));
    }
}
