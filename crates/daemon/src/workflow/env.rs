// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The variables agents MaxHQ starts get: `MAXHQ_AGENT`, so a workflow can
//! bind its run to the agent that started it, and each applying
//! descriptor's `runs.env` once you allowed them.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::Duration;

use maxhq_protocol::ids::AgentId;
use maxhq_workflow::{env_shaped, Catalog, Place, RunRef, Scan};

use super::disk::Disk;
use crate::drive::claude::STRIPPED_PREFIXES;
use crate::drive::Launch;

/// The variable that names the agent to the commands it runs.
pub const AGENT_VAR: &str = "MAXHQ_AGENT";

/// How long an agent's start waits for a scan of a folder nothing follows.
/// A real scan takes milliseconds, and the start holds up every other agent.
const SCAN_WAIT: Duration = Duration::from_secs(2);

/// A variable a workflow's descriptor names for the agents that start in its
/// project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Var {
    /// The name of the workflow that names it.
    pub workflow: String,
    pub name: String,
    pub value: String,
}

/// Each followed folder's descriptor variables, as its last scan found
/// them. Cheap to clone.
#[derive(Clone, Default)]
pub struct Envs(Arc<Inner>);

#[derive(Default)]
struct Inner {
    folders: Mutex<HashMap<PathBuf, Vec<Var>>>,
    library: OnceLock<PathBuf>,
}

impl Envs {
    /// Sets the user library's folder, once.
    pub fn set_library(&self, library: PathBuf) {
        let _ = self.0.library.set(library);
    }

    /// Where descriptors are looked for besides the project.
    pub fn catalog(&self) -> Catalog {
        Catalog {
            library: self.0.library.get().cloned(),
        }
    }

    /// Keeps what a scan of `folder` found.
    pub fn put(&self, folder: &Path, env: Vec<Var>) {
        self.lock().insert(folder.to_owned(), env);
    }

    /// Forgets a folder no longer followed.
    pub fn forget(&self, folder: &Path) {
        self.lock().remove(folder);
    }

    /// The variables for an agent starting in `cwd`: those of the nearest
    /// followed folder holding it, or of a scan of `cwd` itself. A scan past
    /// [`SCAN_WAIT`] goes on alone: this agent starts without its variables,
    /// and later ones get them.
    pub fn for_folder(&self, cwd: &Path) -> Vec<Var> {
        let found = self
            .lock()
            .iter()
            .filter(|(folder, _)| cwd.starts_with(folder))
            .max_by_key(|(folder, _)| folder.as_os_str().len())
            .map(|(_, env)| env.clone());
        if let Some(env) = found {
            return env;
        }
        let (done, scanned) = mpsc::channel();
        let envs = self.clone();
        let folder = cwd.to_owned();
        let scan = move || {
            let scan = envs.catalog().scan(&Disk, &folder);
            let env = of_scan(&scan, &scan.runs(&Disk));
            envs.put(&folder, env.clone());
            let _ = done.send(env);
        };
        let spawned = std::thread::Builder::new()
            .name("workflow-env".into())
            .spawn(scan);
        if spawned.is_err() {
            return Vec::new();
        }
        scanned.recv_timeout(SCAN_WAIT).unwrap_or_else(|_| {
            // Later starts here don't each wait and leave a scan behind.
            self.lock().entry(cwd.to_owned()).or_default();
            Vec::new()
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, Vec<Var>>> {
        self.0.folders.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The variables of every descriptor that applies to the scanned project.
/// The project's own and vendored descriptors always do, and the library's
/// only where its glob found a run. The first descriptor to name a
/// variable wins.
pub fn of_scan(scan: &Scan, runs: &[RunRef]) -> Vec<Var> {
    let mut env: Vec<Var> = Vec::new();
    for workflow in &scan.workflows {
        let desc = &workflow.descriptor;
        let applies = match workflow.place {
            Place::Project | Place::Vendored => true,
            Place::Library => runs.iter().any(|r| r.workflow == desc.id),
            Place::Builtin => false,
        };
        if !applies {
            continue;
        }
        for (name, value) in desc.runs.env.iter() {
            if !fits(name, value) {
                continue;
            }
            if !env.iter().any(|var| var.name == *name) {
                env.push(Var {
                    workflow: desc.name().to_owned(),
                    name: name.to_owned(),
                    value: value.clone(),
                });
            }
        }
    }
    env
}

/// Gives a Claude launch for `agent` its id and `vars`, which you allowed.
/// Every Claude launch comes through here. A variable takes the place of
/// what `launch` set by its name, which its card warns of.
pub fn give(launch: &mut Launch, agent: AgentId, vars: &[Var]) {
    launch
        .env
        .push((AGENT_VAR.into(), agent.to_string().into()));
    for var in vars {
        let name = OsStr::new(&var.name);
        launch.env.retain(|(set, _)| set != name);
        launch.secret_env.retain(|(set, _)| set != name);
        launch.env.push((name.into(), var.value.clone().into()));
    }
}

/// Whether a workflow's variable can go into a launch at all. Loading
/// checked the name, and the agent's own id stays MaxHQ's. These would still
/// break a launch.
fn fits(name: &str, value: &str) -> bool {
    name != AGENT_VAR && env_shaped(name) && !name.contains(['=', '\0']) && !value.contains('\0')
}

/// Whether setting `name` would replace what `launch` sets or undo what it
/// takes out, counting the families a Claude launch scrubs whether or not
/// `maxd` carries one of their names today.
pub fn changes_launch(name: &str, launch: &Launch) -> bool {
    let os_name = OsStr::new(name);
    launch.env.iter().any(|(set, _)| set == os_name)
        || launch.secret_env.iter().any(|(set, _)| set == os_name)
        || launch.env_remove.iter().any(|gone| gone == os_name)
        || STRIPPED_PREFIXES
            .iter()
            .any(|family| name.starts_with(family))
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::Secret;
    use maxhq_workflow::fake::MemFiles;

    use std::ffi::OsString;

    use super::*;
    use crate::drive::Framing;

    fn desc(id: &str, env: &str) -> String {
        format!("format: maxhq.workflow/1.0\nid: {id}\nruns: {{ glob: 'runs/{id}/*/run.yaml', env: {{ {env} }} }}\n")
    }

    #[test]
    fn the_projects_descriptors_apply_and_the_librarys_only_with_a_run() {
        let mut files = MemFiles::default();
        files.put(
            "/work/app/.maxhq/workflows/a.yaml",
            desc("one", "ONE_MODE: a"),
        );
        files.put("/library/b.yaml", desc("two", "TWO_MODE: b"));
        files.put("/library/c.yaml", desc("three", "THREE_MODE: c"));
        files.put("/work/app/runs/two/r1/run.yaml", "run: { id: r1 }\n");
        let catalog = Catalog {
            library: Some("/library".into()),
        };
        let scan = catalog.scan(&files, Path::new("/work/app"));
        let env = of_scan(&scan, &scan.runs(&files));
        assert_eq!(
            env,
            [
                Var {
                    workflow: "one".to_owned(),
                    name: "ONE_MODE".to_owned(),
                    value: "a".to_owned(),
                },
                Var {
                    workflow: "two".to_owned(),
                    name: "TWO_MODE".to_owned(),
                    value: "b".to_owned(),
                },
            ]
        );
    }

    fn launch() -> Launch {
        Launch {
            program: "/bin/claude".into(),
            args: Vec::new(),
            env: Vec::new(),
            env_remove: Vec::new(),
            cwd: "/work/app".into(),
            framing: Framing::ClaudeStreamJson,
            secret_env: Vec::new(),
            run_as: None,
        }
    }

    fn var(name: &str, value: &str) -> Var {
        Var {
            workflow: "one".to_owned(),
            name: name.to_owned(),
            value: value.to_owned(),
        }
    }

    #[test]
    fn a_name_that_skipped_loading_stays_out_of_a_launch() {
        for name in [
            "MAXHQ_AGENT",
            "http_proxy",
            "PYTHONPATH",
            "PATH",
            "ONE_X=1",
            "ONE_\0X",
        ] {
            assert!(!fits(name, "x"), "{name}");
        }
        assert!(!fits("ONE_MODE", "a\0b"));
        // A risky one goes once you allow it: its card marks it.
        for name in [
            "ONE_MODE",
            "ANTHROPIC_BASE_URL",
            "LD_PRELOAD",
            "GIT_SSH_COMMAND",
        ] {
            assert!(fits(name, "x"), "{name}");
        }
    }

    #[test]
    fn an_allowed_variable_replaces_what_a_launch_set() {
        let mut launch = launch();
        launch.env.push(("ONE_SET".into(), "maxhq".into()));
        launch.env_remove.push("ONE_GONE".into());
        launch
            .secret_env
            .push(("ONE_SECRET".into(), Secret::new("maxhq".to_owned())));
        for name in [
            "ONE_SET",
            "ONE_GONE",
            "ONE_SECRET",
            "CLAUDEX_MODE",
            "ANTHROPICX_MODE",
        ] {
            assert!(changes_launch(name, &launch), "{name}");
        }
        assert!(!changes_launch("ONE_MODE", &launch));
        let agent = AgentId::generate();
        give(
            &mut launch,
            agent,
            &[var("ONE_SET", "workflow"), var("ONE_SECRET", "workflow")],
        );
        assert!(launch.secret_env.is_empty());
        assert_eq!(
            launch.env,
            [
                (OsString::from(AGENT_VAR), OsString::from(agent.to_string())),
                (OsString::from("ONE_SET"), OsString::from("workflow")),
                (OsString::from("ONE_SECRET"), OsString::from("workflow")),
            ]
        );
    }

    #[test]
    fn a_launch_names_its_agent_first() {
        let mut launch = launch();
        let agent = AgentId::generate();
        give(&mut launch, agent, &[var("ONE_MODE", "a")]);
        assert_eq!(
            launch.env,
            [
                (OsString::from(AGENT_VAR), OsString::from(agent.to_string())),
                (OsString::from("ONE_MODE"), OsString::from("a")),
            ]
        );
    }

    #[test]
    fn a_scan_that_doesnt_end_doesnt_hold_up_an_agents_start() {
        let envs = Envs::default();
        let dir = crate::testing::TempDir::new();
        let workflows = dir.path().join(maxhq_workflow::WORKFLOWS_DIR);
        std::fs::create_dir_all(&workflows).unwrap();
        // Reading a pipe nothing writes to never ends.
        let made = std::process::Command::new("mkfifo")
            .arg(workflows.join("x.yaml"))
            .status();
        assert!(made.unwrap().success());
        let started = std::time::Instant::now();
        assert!(envs.for_folder(dir.path()).is_empty());
        assert!(started.elapsed() < SCAN_WAIT * 3);
        // The next start doesn't wait again.
        let started = std::time::Instant::now();
        assert!(envs.for_folder(dir.path()).is_empty());
        assert!(started.elapsed() < SCAN_WAIT / 2);
    }

    #[test]
    fn an_unfollowed_folders_scan_gives_its_variables() {
        let envs = Envs::default();
        let dir = crate::testing::TempDir::new();
        let workflows = dir.path().join(maxhq_workflow::WORKFLOWS_DIR);
        std::fs::create_dir_all(&workflows).unwrap();
        let descriptor = desc("one", "ONE_MODE: a");
        std::fs::write(workflows.join("one.yaml"), descriptor).unwrap();
        let env = envs.for_folder(dir.path());
        assert_eq!(env, vec![var("ONE_MODE", "a")]);
    }

    #[test]
    fn a_followed_folder_answers_for_the_folders_under_it() {
        let envs = Envs::default();
        let dir = crate::testing::TempDir::new();
        let root = dir.path().join("app");
        envs.put(&root, vec![var("ONE_MODE", "a")]);
        assert_eq!(envs.for_folder(&root.join("sub")).len(), 1);
        envs.forget(&root);
        assert!(envs.for_folder(&root.join("sub")).is_empty());
    }
}
