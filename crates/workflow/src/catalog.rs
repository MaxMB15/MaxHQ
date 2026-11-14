// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Finding a project's workflows and their runs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use globset::GlobBuilder;
use maxhq_protocol::ids::AgentId;
use maxhq_protocol::workflow::{RunNotice, Tone};
use serde_norway::Value as Yaml;

use crate::descriptor::Descriptor;
use crate::eval::read_capped;
use crate::inputs::{AgentFacts, Files};
use crate::load::{finish, merge, read, LoadError, Raw};

/// Where descriptors live under a project or library folder.
pub const WORKFLOWS_DIR: &str = ".maxhq/workflows";
/// Folders never looked in.
const SKIP: [&str; 2] = [".git", "node_modules"];
/// How deep a `**` goes below the glob's fixed start.
const MAX_DEPTH: usize = 8;
/// The most folder entries one glob looks at.
const MAX_ENTRIES: usize = 20_000;

/// Where a descriptor came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// `<project>/.maxhq/workflows`.
    Project,
    /// `<project>/<dir>/.maxhq/workflows`, for a vendored workflow.
    Vendored,
    /// The user library.
    Library,
    /// Built into MaxHQ.
    Builtin,
}

/// Where MaxHQ looks for descriptors besides the project.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    /// The user library's `workflows/` folder, in the app data folder.
    pub library: Option<PathBuf>,
}

/// A workflow that loaded.
#[derive(Clone, Debug)]
pub struct Workflow {
    /// Its descriptor, overlays merged.
    pub descriptor: Arc<Descriptor>,
    /// Where it came from.
    pub place: Place,
    /// Its file, none for a built-in.
    pub path: Option<PathBuf>,
    /// The folder holding its `.maxhq/`, for `{workflow}`.
    pub root: PathBuf,
}

/// A file that didn't load.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    /// The file.
    pub path: PathBuf,
    /// Why.
    pub error: LoadError,
}

/// What a scan found in one project.
#[derive(Clone, Debug)]
pub struct Scan {
    /// The project's root.
    pub project: PathBuf,
    /// Each workflow once, first found first, `maxhq.agents` last.
    pub workflows: Vec<Workflow>,
    /// Files that didn't load.
    pub problems: Vec<Problem>,
}

/// One run a workflow's glob matched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRef {
    /// The workflow, by id.
    pub workflow: String,
    /// The run file.
    pub file: PathBuf,
    /// The project's root.
    pub project: PathBuf,
    /// The folder holding the workflow's `.maxhq/`.
    pub root: PathBuf,
}

impl Catalog {
    /// Finds the workflows for a project: its own, then vendored ones, then
    /// the library's, then `maxhq.agents`. The first with an id wins, and
    /// overlays from any of them merge over it in the same order.
    pub fn scan(&self, files: &dyn Files, project: &Path) -> Scan {
        let mut places = vec![(
            project.join(WORKFLOWS_DIR),
            project.to_owned(),
            Place::Project,
        )];
        let mut dirs: Vec<String> = files
            .list(project)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.dir && !SKIP.contains(&e.name.as_str()))
            .map(|e| e.name)
            .collect();
        dirs.sort();
        for dir in dirs {
            let root = project.join(dir);
            places.push((root.join(WORKFLOWS_DIR), root, Place::Vendored));
        }
        if let Some(library) = &self.library {
            places.push((library.clone(), library.clone(), Place::Library));
        }

        let mut found: Vec<(String, u32, Yaml, Place, PathBuf, PathBuf)> = Vec::new();
        let mut overlays: Vec<(String, Yaml)> = Vec::new();
        let mut problems = Vec::new();
        for (dir, root, place) in places {
            let mut names: Vec<String> = files
                .list(&dir)
                .unwrap_or_default()
                .into_iter()
                .filter(|e| {
                    !e.dir
                        && [".yaml", ".yml", ".json"]
                            .iter()
                            .any(|x| e.name.ends_with(x))
                })
                .map(|e| e.name)
                .collect();
            names.sort();
            for name in names {
                let path = dir.join(name);
                let Some(bytes) = read_capped(files, &path) else {
                    continue;
                };
                match read(&bytes) {
                    Ok(Raw::Workflow { id, minor, value }) => {
                        if !found.iter().any(|f| f.0 == id) {
                            found.push((id, minor, value, place, path, root.clone()));
                        }
                    }
                    Ok(Raw::Overlay { workflow, value }) => overlays.push((workflow, value)),
                    Err(error) => problems.push(Problem { path, error }),
                }
            }
        }

        let mut workflows = Vec::new();
        for (id, minor, mut value, place, path, root) in found {
            for (_, patch) in overlays.iter().filter(|(w, _)| *w == id) {
                merge(&mut value, patch.clone());
            }
            match finish(value, minor) {
                Ok(descriptor) => workflows.push(Workflow {
                    descriptor: Arc::new(descriptor),
                    place,
                    path: Some(path),
                    root,
                }),
                Err(error) => problems.push(Problem { path, error }),
            }
        }
        workflows.push(Workflow {
            descriptor: Arc::new(Descriptor::agents()),
            place: Place::Builtin,
            path: None,
            root: project.to_owned(),
        });
        Scan {
            project: project.to_owned(),
            workflows,
            problems,
        }
    }
}

impl Scan {
    /// A workflow by id.
    pub fn workflow(&self, id: &str) -> Option<&Workflow> {
        self.workflows.iter().find(|w| w.descriptor.id == id)
    }

    /// The notice for runs the fallback draws because a descriptor needs a
    /// newer MaxHQ.
    pub fn fallback_notice(&self) -> Option<RunNotice> {
        self.problems
            .iter()
            .find(|p| matches!(p.error, LoadError::TooNew { .. }))
            .map(|p| RunNotice {
                text: p.error.to_string(),
                tone: Tone::Warn,
            })
    }

    /// Every run file a workflow's glob matches, each once, for the first
    /// workflow that matches it.
    pub fn runs(&self, files: &dyn Files) -> Vec<RunRef> {
        let mut runs: Vec<RunRef> = Vec::new();
        for workflow in &self.workflows {
            let glob = &workflow.descriptor.runs.glob;
            if glob.is_empty() {
                continue;
            }
            for file in expand(files, &self.project, glob) {
                if !runs.iter().any(|r| r.file == file) {
                    runs.push(RunRef {
                        workflow: workflow.descriptor.id.clone(),
                        file,
                        project: self.project.clone(),
                        root: workflow.root.clone(),
                    });
                }
            }
        }
        runs
    }
}

/// The leads with sub-agents that no run has bound, oldest first, for
/// `maxhq.agents` to draw.
pub fn fallback_leads(agents: &[AgentFacts], taken: &[AgentId]) -> Vec<AgentId> {
    let mut leads: Vec<&AgentFacts> = agents
        .iter()
        .filter(|f| f.lead.is_none() && !taken.contains(&f.id))
        .filter(|f| agents.iter().any(|sub| sub.lead == Some(f.id)))
        .collect();
    leads.sort_by_key(|f| f.started);
    leads.into_iter().map(|f| f.id).collect()
}

fn has_meta(part: &str) -> bool {
    part.contains(['*', '?', '[', '{'])
}

/// The files under the project a glob matches, sorted.
fn expand(files: &dyn Files, project: &Path, glob: &str) -> Vec<PathBuf> {
    let glob = glob.strip_prefix("{project}/").unwrap_or(glob);
    let parts: Vec<&str> = glob.split('/').collect();
    let fixed = parts.iter().take_while(|p| !has_meta(p)).count();
    let base = parts[..fixed]
        .iter()
        .fold(project.to_owned(), |path, part| path.join(part));
    if fixed == parts.len() {
        return files.stat(&base).map(|_| vec![base]).unwrap_or_default();
    }
    let Ok(matcher) = GlobBuilder::new(glob).literal_separator(true).build() else {
        return Vec::new();
    };
    let matcher = matcher.compile_matcher();
    let depth = if parts[fixed..].contains(&"**") {
        MAX_DEPTH
    } else {
        parts.len() - fixed
    };
    let mut out = Vec::new();
    let mut seen = 0;
    let mut todo = vec![(base, 0)];
    while let Some((dir, level)) = todo.pop() {
        for entry in files.list(&dir).unwrap_or_default() {
            seen += 1;
            if seen > MAX_ENTRIES {
                break;
            }
            let path = dir.join(&entry.name);
            if entry.dir {
                if level + 1 < depth && !SKIP.contains(&entry.name.as_str()) {
                    todo.push((path, level + 1));
                }
            } else if path
                .strip_prefix(project)
                .is_ok_and(|rel| matcher.is_match(rel))
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeAgents, MemFiles};
    use crate::inputs::Agents;

    const PROJECT: &str = "/work/example-app";

    fn desc(id: &str, glob: &str, extra: &str) -> String {
        format!("format: maxhq.workflow/1.0\nid: {id}\nruns: {{ glob: '{glob}' }}\n{extra}")
    }

    fn at(rel: &str) -> PathBuf {
        Path::new(PROJECT).join(rel)
    }

    #[test]
    fn finds_workflows_in_all_four_places_first_one_winning() {
        let mut files = MemFiles::default();
        files.put(
            at(".maxhq/workflows/a.yaml"),
            desc("one", "runs/*/run.yaml", "name: Project's"),
        );
        files.put(
            at(".vendor/.maxhq/workflows/b.yaml"),
            desc("one", "x", "name: Vendored"),
        );
        files.put(at(".vendor/.maxhq/workflows/c.yaml"), desc("two", "x", ""));
        files.put(
            at("node_modules/.maxhq/workflows/d.yaml"),
            desc("three", "x", ""),
        );
        files.put(at(".git/.maxhq/workflows/d.yaml"), desc("three", "x", ""));
        files.put("/data/workflows/e.yml", desc("four", "x", ""));
        files.put("/data/workflows/notes.txt", "ignored");
        let catalog = Catalog {
            library: Some("/data/workflows".into()),
        };
        let scan = catalog.scan(&files, Path::new(PROJECT));
        let ids: Vec<(&str, Place)> = scan
            .workflows
            .iter()
            .map(|w| (w.descriptor.id.as_str(), w.place))
            .collect();
        assert_eq!(
            ids,
            [
                ("one", Place::Project),
                ("two", Place::Vendored),
                ("four", Place::Library),
                ("maxhq.agents", Place::Builtin),
            ]
        );
        assert_eq!(scan.workflow("one").unwrap().descriptor.name(), "Project's");
        assert_eq!(scan.workflow("two").unwrap().root, at(".vendor"));
        assert!(scan.problems.is_empty());
    }

    #[test]
    fn overlays_from_the_library_apply_and_bad_files_are_problems() {
        let mut files = MemFiles::default();
        files.put(
            at(".vendor/.maxhq/workflows/a.yaml"),
            desc("one", "x", "name: Theirs"),
        );
        files.put(
            "/data/workflows/mine.yaml",
            "format: maxhq.workflow-overlay/1.0\nworkflow: one\nname: Mine\n",
        );
        files.put(
            at(".maxhq/workflows/bad.yaml"),
            "format: maxhq.workflow/1.0\nid: bad\n",
        );
        files.put(
            at(".maxhq/workflows/new.yaml"),
            desc("new", "x", "").replace("/1.0", "/2.0"),
        );
        let catalog = Catalog {
            library: Some("/data/workflows".into()),
        };
        let scan = catalog.scan(&files, Path::new(PROJECT));
        assert_eq!(scan.workflow("one").unwrap().descriptor.name(), "Mine");
        assert_eq!(scan.problems.len(), 2);
        assert!(scan.workflow("new").is_none());
        assert_eq!(
            scan.fallback_notice().unwrap().text,
            "This workflow needs a newer MaxHQ."
        );
    }

    #[test]
    fn a_newer_major_falls_back_to_the_agents_view() {
        let mut files = MemFiles::default();
        files.put(
            at(".maxhq/workflows/new.yaml"),
            desc("new", "runs/*/run.yaml", "").replace("/1.0", "/2.0"),
        );
        files.put(at("runs/r1/run.yaml"), "nodes: []");
        let scan = Catalog::default().scan(&files, Path::new(PROJECT));
        assert!(scan.runs(&files).is_empty(), "nothing reads its runs");
        let mut agents = FakeAgents::default();
        let lead = agents.add("lead", None);
        agents.add("sub", Some(lead));
        let leads = fallback_leads(&agents.list(), &[]);
        assert_eq!(leads, [lead]);
        let mut run = crate::Run::lead(
            maxhq_protocol::ids::DeviceId::derive("d"),
            maxhq_protocol::ids::ProjectId::derive("p"),
            Path::new(PROJECT),
            lead,
        );
        run.set_notice(scan.fallback_notice());
        let fallback = scan.workflow("maxhq.agents").unwrap();
        let s = run.derive(&fallback.descriptor, &files, &agents);
        assert_eq!(s.notice.unwrap().text, "This workflow needs a newer MaxHQ.");
        assert_eq!(s.nodes.len(), 1);
    }

    #[test]
    fn globs_match_runs_once_each() {
        let mut files = MemFiles::default();
        files.put(
            at(".maxhq/workflows/a.yaml"),
            desc("one", "{project}/.state/*/task.yaml", ""),
        );
        files.put(
            at(".maxhq/workflows/b.yaml"),
            desc("two", "**/task.yaml", ""),
        );
        files.put(at(".state/t-1/task.yaml"), "a: 1");
        files.put(at(".state/t-2/task.yaml"), "a: 1");
        files.put(at(".state/t-2/deeper/task.yaml"), "a: 1");
        files.put(at("node_modules/x/task.yaml"), "a: 1");
        files.put(at("task.yaml"), "a: 1");
        let scan = Catalog::default().scan(&files, Path::new(PROJECT));
        let runs: Vec<(String, PathBuf)> = scan
            .runs(&files)
            .into_iter()
            .map(|r| (r.workflow, r.file))
            .collect();
        assert_eq!(
            runs,
            [
                ("one".to_string(), at(".state/t-1/task.yaml")),
                ("one".to_string(), at(".state/t-2/task.yaml")),
                ("two".to_string(), at(".state/t-2/deeper/task.yaml")),
                ("two".to_string(), at("task.yaml")),
            ]
        );
    }

    #[test]
    fn fallback_leads_skip_bound_ones_and_leads_alone() {
        let mut agents = FakeAgents::default();
        let a = agents.add("a", None);
        let b = agents.add("b", None);
        agents.add("alone", None);
        agents.add("sub", Some(a));
        agents.add("sub", Some(b));
        assert_eq!(fallback_leads(&agents.list(), &[a]), [b]);
    }
}
