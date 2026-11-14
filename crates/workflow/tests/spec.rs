// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Holds the standard's examples, and every fixture under `tests/fixtures`,
//! to the checked-in schemas, and checks the engine reads the examples the
//! way `spec.md` says.

use std::fs;
use std::path::{Path, PathBuf};

use jsonschema::Validator;
use maxhq_protocol::ids::{DeviceId, ProjectId};
use maxhq_protocol::workflow::{NodeStatus, RunSnapshot};
use maxhq_workflow::fake::MemFiles;
use maxhq_workflow::{schema, Catalog, NoAgents, Run, Scan};
use serde_json::{json, Value as Json};

/// Folders whose files are checked, from the repository's root. A folder
/// that doesn't exist yet is passed over.
const CHECKED: [&str; 2] = [
    "spec/workflow/v1/examples",
    "crates/workflow/tests/fixtures",
];

/// The modeline that names a YAML file's schema.
const MODELINE: &str = "# yaml-language-server: $schema=";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn examples() -> PathBuf {
    repo().join(CHECKED[0])
}

/// Which schema a file follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Descriptor,
    Run,
    Event,
}

impl Kind {
    fn file(self) -> &'static str {
        match self {
            Self::Descriptor => "descriptor.schema.json",
            Self::Run => "run.schema.json",
            Self::Event => "event.schema.json",
        }
    }
}

/// A validator per schema, built from the checked-in files rather than the
/// types, so what's published is what's tested. The drift test keeps the
/// two the same.
struct Schemas(Vec<(Kind, Validator)>);

impl Schemas {
    fn load() -> Self {
        let dir = repo().join(schema::SCHEMA_DIR);
        Self(
            [Kind::Descriptor, Kind::Run, Kind::Event]
                .into_iter()
                .map(|kind| {
                    let text = fs::read_to_string(dir.join(kind.file())).expect("a schema");
                    let schema: Json = serde_json::from_str(&text).expect("schema JSON");
                    let validator = jsonschema::options()
                        .should_validate_formats(true)
                        // Patterns are Rust regex syntax, which the engine
                        // reads, not ECMA-262.
                        .with_format("regex", |text: &str| regex::Regex::new(text).is_ok())
                        .build(&schema)
                        .expect("a valid schema");
                    (kind, validator)
                })
                .collect(),
        )
    }

    fn errors(&self, kind: Kind, value: &Json) -> Vec<String> {
        let (_, validator) = self.0.iter().find(|(k, _)| *k == kind).expect("a schema");
        validator
            .iter_errors(value)
            .map(|e| format!("{} at {}", e, e.instance_path()))
            .collect()
    }
}

/// Every file under a folder, hidden ones included, sorted.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_owned()];
    while let Some(dir) = todo.pop() {
        for entry in fs::read_dir(&dir).expect("a folder") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                todo.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// The schema a file follows, if any: the one its modeline names, else a
/// descriptor for a file whose `format` says so, else an event log for
/// `.jsonl`. Mapped run files and other logs follow no schema.
fn kind(path: &Path, text: &str) -> Result<Option<Kind>, String> {
    if let Some(url) = text.lines().find_map(|l| l.strip_prefix(MODELINE)) {
        let name = url
            .strip_prefix(schema::SCHEMA_BASE)
            .ok_or_else(|| format!("names a schema outside the standard: {url}"))?;
        return [Kind::Descriptor, Kind::Run, Kind::Event]
            .into_iter()
            .find(|k| k.file() == name)
            .map(Some)
            .ok_or_else(|| format!("names no schema of the standard: {name}"));
    }
    if path.extension().is_some_and(|x| x == "jsonl") {
        return Ok(Some(Kind::Event));
    }
    let doc = parse(path, text).ok().flatten();
    let format = doc
        .as_ref()
        .and_then(|d| d.get("format")?.as_str().map(str::to_owned));
    Ok(format
        .filter(|f| f.starts_with("maxhq.workflow/"))
        .map(|_| Kind::Descriptor))
}

/// A YAML or JSON file as JSON, `None` for other files.
fn parse(path: &Path, text: &str) -> Result<Option<Json>, String> {
    let ext = path
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or_default();
    match ext {
        "json" => serde_json::from_str(text)
            .map(Some)
            .map_err(|e| e.to_string()),
        "yaml" | "yml" => serde_norway::from_str(text)
            .map(Some)
            .map_err(|e| e.to_string()),
        _ => Ok(None),
    }
}

/// What's wrong with one file, and the schema it was checked against.
fn check(schemas: &Schemas, path: &Path) -> (Option<Kind>, Vec<String>) {
    let Ok(text) = fs::read_to_string(path) else {
        return (None, Vec::new());
    };
    let kind = match kind(path, &text) {
        Ok(Some(kind)) => kind,
        Ok(None) => return (None, Vec::new()),
        Err(error) => return (None, vec![error]),
    };
    let mut errors = Vec::new();
    if kind == Kind::Event {
        // Only whole lines are events; a last line with no newline is still
        // being written.
        let whole = text.rsplit_once('\n').map_or("", |(lines, _)| lines);
        for (n, line) in whole.lines().enumerate() {
            match serde_json::from_str::<Json>(line) {
                Ok(value) => errors.extend(
                    schemas
                        .errors(kind, &value)
                        .into_iter()
                        .map(|e| format!("line {}: {e}", n + 1)),
                ),
                Err(e) => errors.push(format!("line {}: {e}", n + 1)),
            }
        }
        return (Some(kind), errors);
    }
    match parse(path, &text) {
        Ok(Some(value)) => errors.extend(schemas.errors(kind, &value)),
        Ok(None) => errors.push("isn't YAML or JSON".to_owned()),
        Err(e) => errors.push(e),
    }
    if kind == Kind::Descriptor
        && let Err(e) = maxhq_workflow::load(text.as_bytes())
    {
        errors.push(format!("doesn't load: {e}"));
    }
    (Some(kind), errors)
}

#[test]
fn every_example_and_fixture_follows_its_schema() {
    let schemas = Schemas::load();
    let mut failures = Vec::new();
    let mut seen = Vec::new();
    for dir in CHECKED
        .map(|d| repo().join(d))
        .iter()
        .filter(|d| d.is_dir())
    {
        for path in files_under(dir) {
            let (kind, errors) = check(&schemas, &path);
            if dir == &examples() {
                seen.extend(kind);
            }
            failures.extend(
                errors
                    .into_iter()
                    .map(|e| format!("{}: {e}", path.display())),
            );
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    for kind in [Kind::Descriptor, Kind::Run, Kind::Event] {
        assert!(seen.contains(&kind), "no example follows {}", kind.file());
    }
}

#[test]
fn only_extension_keys_may_be_added() {
    let schemas = Schemas::load();
    let path = examples().join("reference/.maxhq/workflows/release.yaml");
    let text = fs::read_to_string(&path).unwrap();
    let base = parse(&path, &text).unwrap().unwrap();
    let with = |key: &str| {
        let mut doc = base.clone();
        doc[key] = json!({ "a": 1 });
        doc
    };
    assert!(schemas.errors(Kind::Descriptor, &base).is_empty());
    assert!(schemas
        .errors(Kind::Descriptor, &with("x-notes"))
        .is_empty());
    assert!(!schemas.errors(Kind::Descriptor, &with("notes")).is_empty());
}

#[test]
fn a_predicate_names_its_node_by_id_or_node_but_not_both() {
    let schemas = Schemas::load();
    let path = examples().join("mapped/.maxhq/workflows/pipeline.yaml");
    let text = fs::read_to_string(&path).unwrap();
    let base = parse(&path, &text).unwrap().unwrap();
    let with = |when: Json| {
        let mut doc = base.clone();
        doc["rules"][0]["when"] = when;
        doc
    };
    assert!(schemas
        .errors(Kind::Descriptor, &with(json!({ "id": "docs" })))
        .is_empty());
    assert!(!schemas
        .errors(
            Kind::Descriptor,
            &with(json!({ "id": "docs", "node": "docs" }))
        )
        .is_empty());
}

#[test]
fn a_schema_naming_modeline_must_name_one_of_the_standard() {
    let path = Path::new("run.yaml");
    let other = format!("{MODELINE}https://example.com/other.schema.json\n");
    assert!(kind(path, &other).is_err());
    let unknown = format!("{MODELINE}{}nope.schema.json\n", schema::SCHEMA_BASE);
    assert!(kind(path, &unknown).is_err());
}

/// One example folder as a project: what the scan found and each run
/// derived once, with no agents.
fn derive(name: &str) -> (Scan, Vec<(RunSnapshot, Run)>) {
    let project = examples().join(name).canonicalize().unwrap();
    let files = MemFiles::from_dir(&project).unwrap();
    let scan = Catalog::default().scan(&files, &project);
    assert!(scan.problems.is_empty(), "{:?}", scan.problems);
    let runs = scan
        .runs(&files)
        .into_iter()
        .map(|run_ref| {
            let desc = scan.workflow(&run_ref.workflow).unwrap().descriptor.clone();
            let mut run = Run::new(DeviceId::derive("d"), ProjectId::derive("p"), run_ref);
            let snapshot = run.derive(&desc, &files, &NoAgents);
            assert_eq!(snapshot.notice, None, "{name}");
            (snapshot, run)
        })
        .collect::<Vec<_>>();
    assert_eq!(runs.len(), 1, "{name} has one run");
    (scan, runs)
}

fn statuses(snapshot: &RunSnapshot) -> Vec<(&str, NodeStatus)> {
    snapshot
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n.status))
        .collect()
}

fn node<'a>(snapshot: &'a RunSnapshot, id: &str) -> &'a maxhq_protocol::workflow::NodeState {
    snapshot.nodes.iter().find(|n| n.id == id).unwrap()
}

fn texts(badges: &[maxhq_protocol::workflow::Badge]) -> Vec<&str> {
    badges.iter().map(|b| b.text.as_str()).collect()
}

#[test]
fn the_reference_example_reads_as_the_spec_says() {
    use NodeStatus::*;
    let (_, runs) = derive("reference");
    let (snapshot, run) = &runs[0];
    assert_eq!(snapshot.key.run, "r-0142");
    assert_eq!(snapshot.header.label, "Release 0.4.2");
    assert_eq!(
        statuses(snapshot),
        [
            ("plan", Done),
            ("build", Done),
            ("unit", Busy),
            ("lint", Done),
            ("review", Queued),
            ("notes", Ready),
            ("publish", Queued),
        ]
    );
    assert_eq!(texts(&node(snapshot, "build").badges), ["try 2"]);
    assert_eq!(node(snapshot, "build").attempts.len(), 2);
    assert_eq!(snapshot.loops[0].count, 1);
    assert_eq!(texts(run.header_lines()), ["2 checks"]);
}

#[test]
fn the_mapped_example_reads_as_the_spec_says() {
    use NodeStatus::*;
    let (_, runs) = derive("mapped");
    let (snapshot, run) = &runs[0];
    assert_eq!(snapshot.key.run, "job-7");
    assert_eq!(snapshot.header.status, Busy);
    assert_eq!(
        statuses(snapshot),
        [
            ("research", Done),
            ("design", Done),
            ("approve", WaitingHuman),
            ("api", Busy),
            ("code", Blocked),
            ("docs", WillSkip),
            ("verify", Queued),
            ("review", Omitted),
        ]
    );
    let docs = node(snapshot, "docs");
    assert!(docs.provisional);
    assert_eq!(docs.note.as_deref(), Some("Quick jobs skip the docs."));
    let code = node(snapshot, "code");
    assert_eq!(code.note.as_deref(), Some("Out of retries after 3 tries."));
    assert_eq!(texts(&code.badges), ["2 errors"]);
    assert_eq!(code.attempts.len(), 3);
    assert_eq!(texts(&node(snapshot, "api").badges), ["chunk 2/5"]);
    // The preview's pid isn't running, so its link stays hidden.
    assert!(node(snapshot, "api").links.is_empty());
    assert!(node(snapshot, "approve").note.is_some());
    assert_eq!(
        texts(run.header_lines()),
        ["Effort: quick", "2 retries so far", "5 steps left"]
    );
}

#[test]
fn the_vendored_example_reads_as_the_spec_says() {
    use NodeStatus::*;
    let (scan, runs) = derive("vendored");
    let checks = scan.workflow("checks").unwrap();
    assert_eq!(checks.descriptor.name(), "Pre-merge checks");
    assert_eq!(checks.descriptor.revision, None);
    let (snapshot, run) = &runs[0];
    assert_eq!(
        statuses(snapshot),
        [
            ("fmt", Done),
            ("clippy", Failed),
            ("unit", Skipped),
            ("e2e", Ready),
            ("docs", Unknown),
        ]
    );
    assert_eq!(snapshot.header.status, Failed);
    let lint = snapshot
        .shape
        .groups
        .iter()
        .find(|g| g.id == "lint")
        .unwrap();
    assert!(lint.collapsed);
    assert_eq!(texts(run.header_lines()), ["checks 1.4.0"]);
}
