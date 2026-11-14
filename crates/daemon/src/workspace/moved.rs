// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The copy side of a workspace move. The copy is an `mhq-` container, so
//! it's MaxHQ's from the start: this sets it up the way a create would,
//! with the old scripts' leftovers removed first, and resumes the
//! original's agent in it. Nothing here touches the original.

use futures::future::BoxFuture;
use maxhq_core::names;
use maxhq_protocol::ids::{AccountId, ProjectId, WorkspaceId};
use maxhq_protocol::model::{
    EgressCheck, GuestState, PermissionMode, Vendor, WorkspaceLocation, WorkspaceState,
};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use tracing::{info, warn};

use super::create::{egress_problem, step_error};
use super::exec::IncusExec;
use super::keys::{Keys, Record};
use super::manager::Manager;
use super::repo::Repo;
use super::snapshots::unsaved_parts;
use crate::incus::moves::AUTOSTART;
use crate::incus::{InstanceName, StateAction};
use crate::moves::{
    claude_folder as project_folder, copy_path, top_folder, vendor_name, CopySetup, Host,
    Record as Move, Resume,
};
use crate::recipe::{self, retire, Exec, ExecUser};
use crate::session::{ResumeMoved, Sessions};

/// Copies what a CLI reads from its own folder, where the old scripts ran it,
/// to the account's folder MaxHQ runs it with, or the CLI would stop reading
/// it: your instructions and settings, the session's transcript and each
/// project's memory. Other transcripts stay behind.
///
/// Claude files a transcript and a memory under the folder it runs in, and
/// that folder changed if the repo moved, so they land under the new name.
/// What the account's folder has already stays.
const CARRY_OVER: &str = r#"set -eu
home=$1; src=$2; dst=$3; id=$4; project=$5
cd "$src" 2>/dev/null || exit 0
mkdir -p "$dst"
folder() { printf %s "$1" | sed 's/[^A-Za-z0-9]/-/g'; }
for file in CLAUDE.md AGENTS.md settings.json; do
  if [ -f "$file" ] && [ ! -e "$dst/$file" ]; then
    cp -pL "$file" "$dst/$file"
  fi
done
if [ -d projects ]; then
  find projects -type f -name "*$id*.jsonl" | while read -r file; do
    mkdir -p "$dst/projects/$project"
    cp -p "$file" "$dst/projects/$project/"
  done
fi
if [ -d sessions ]; then
  find sessions -type f -name "*$id*.jsonl" | while read -r file; do
    mkdir -p "$dst/$(dirname "$file")"
    cp -p "$file" "$dst/$file"
  done
fi
old=$(folder "$home/")
new=$(folder "$home/work/")
moved=
for link in "$home"/*; do
  if [ -L "$link" ]; then
    case $(readlink "$link") in
      work/*) moved="$moved $(folder "$(basename "$link")")" ;;
    esac
  fi
done
for memory in projects/*/memory; do
  [ -d "$memory" ] || continue
  name=${memory#projects/}
  name=${name%/memory}
  to=$name
  for part in $moved; do
    case $name in
      "$old$part" | "$old$part-"*) to="$new${name#"$old"}" ;;
    esac
  done
  if [ ! -e "$dst/projects/$to/memory" ]; then
    mkdir -p "$dst/projects/$to"
    cp -pR "$memory" "$dst/projects/$to/memory"
  fi
done
"#;

/// Moves each repo in the home folder, and the folder `$2` names, to the work
/// folder, and leaves a link at each old path, so anything that names the old
/// path still finds it. A second run finds the links and does nothing.
const MOVE_REPOS: &str = r#"set -eu
cd "$1"
top=$2
mkdir -p work
move() {
  case $1 in '' | work) return 0 ;; esac
  [ -e "$1" ] && [ ! -L "$1" ] || return 0
  if [ -e "work/$1" ]; then
    echo "work/$1 is already there" >&2
    exit 1
  fi
  mv -- "$1" "work/$1"
  ln -s "work/$1" "$1"
}
for dir in */; do
  [ ! -e "$dir.git" ] || move "${dir%/}"
done
move "$top"
"#;

/// Keeps the commit name and email the old setup put in `/etc/gitconfig`,
/// which MaxHQ's recipe replaces, by copying them to the home folder's config.
/// A setting already there stands, and a second run finds nothing to copy.
const KEEP_GIT_SETTINGS: &str = r#"
set -eu
config=$1/.gitconfig
for key in user.name user.email init.defaultBranch; do
  value=$(git config --system --get "$key") || continue
  git config --file "$config" --get "$key" > /dev/null || git config --file "$config" "$key" "$value"
done
"#;

/// The rest of `maxd` a move works with.
pub struct MoveHost {
    pub manager: Manager,
    pub sessions: Sessions,
}

impl MoveHost {
    fn name(record: &Move) -> Result<InstanceName, String> {
        InstanceName::new(&record.target).map_err(|error| error.to_string())
    }

    async fn setup(&self, record: &Move) -> Result<CopySetup, String> {
        let manager = &self.manager;
        let name = Self::name(record)?;
        let device = manager.config().hub.device().id;
        let (network, _) = manager.place().await?;
        manager.config().caps.check_parent()?;
        manager.guest_binary()?;
        manager
            .ensure_guests_dir()
            .map_err(|error| error.to_string())?;
        let path = copy_path(&record.found)
            .ok_or("The look found no repo in the original, so there's nothing to move.")?;
        let repo = record.found.repo.as_deref().and_then(Repo::parse);
        let project = match &repo {
            Some(repo) => repo.project(device),
            None => names::project(device, &path, None).id,
        };
        let title = match manager.config().hub.get(&EntityKey::Project(project)) {
            Some(Entity::Project(project)) => project.name,
            _ => record
                .found
                .title
                .clone()
                .or_else(|| repo.as_ref().map(|repo| repo.title(device)))
                .unwrap_or_else(|| record.target.clone()),
        };
        let workspace = names::workspace(
            device,
            &WorkspaceLocation::Container {
                instance: name.to_string(),
                path: path.clone(),
            },
        );
        let keys = Keys {
            project,
            device,
            workspace,
            title,
            repo: repo.map(|repo| repo.clone_url()).unwrap_or_default(),
            boot: record.found.autostart == "true",
            recipe: 0,
            agents: Vec::new(),
            created: Timestamp::now(),
            path: Some(path),
        };
        let mut config = keys.encode();
        let memory = manager.memory_soft(project);
        config.extend(manager.config().caps.create_keys(name.as_str(), memory));
        config.insert(AUTOSTART.to_owned(), "false".to_owned());
        Ok(CopySetup {
            workspace,
            config,
            network,
            guest_socket: manager.socket(&name),
        })
    }

    async fn converge_copy(&self, record: &Move) -> Result<(), String> {
        let manager = &self.manager;
        let name = Self::name(record)?;
        let Some(found) = manager
            .incus()
            .instance(&name)
            .await
            .map_err(|e| e.to_string())?
        else {
            return Err(format!("{name} isn't there. Copy again."));
        };
        let keys = Keys::decode(&found.config)
            .map_err(|error| format!("{name}'s keys don't read: {error}"))?;
        let workspace = keys.workspace;
        let project = keys.project;
        let Some(_op) = manager.begin(workspace) else {
            return Err(format!(
                "{name} is already being changed. Wait for it to finish."
            ));
        };
        let (network, pool) = manager.place().await?;
        let steps = (manager.config().steps)(0);
        let of = 7 + steps.len() as u32;
        let mut done = 0;
        let step = |text: &str, done: &mut u32| {
            manager.set_state(
                workspace,
                WorkspaceState::Creating {
                    step: text.to_owned(),
                    done: *done,
                    of,
                },
            );
            *done += 1;
        };
        manager.put(Record {
            instance: name.to_string(),
            keys,
            state: WorkspaceState::Creating {
                step: "Setting up the copy".to_owned(),
                done: 0,
                of,
            },
            guest: GuestState::Installing,
            egress: EgressCheck::NotRun,
            egress_note: None,
            pool: Some(pool.clone()),
            network: Some(network.clone()),
            host_boot: Some(manager.inner.host_boot.clone()),
            ssh_host_key: None,
            memory_soft: manager.memory_soft(project),
            unsaved: None,
        });
        let result = async {
            let incus = manager.incus();
            let timing = manager.config().timing;
            step("Setting up the copy", &mut done);
            let socket = manager.socket(&name);
            incus
                .own_devices(&name, &pool, &network, &socket)
                .await
                .map_err(|e| e.to_string())?;
            incus
                .set_state(&name, StateAction::Start, timing.state_timeout)
                .await
                .map_err(|e| e.to_string())?;
            let exec = IncusExec::new(incus.clone(), name.clone());
            manager.wait_ready(&exec).await?;

            step("Putting maxd in the copy", &mut done);
            let binary = manager.guest_binary()?;
            super::install::install(incus, &name, &binary)
                .await
                .map_err(|e| e.to_string())?;

            step("Checking egress", &mut done);
            let check = manager.check_egress(workspace, &exec, &network).await;
            if let Some(problem) = manager
                .record(workspace)
                .and_then(|r| egress_problem(r.egress_note.as_deref(), &check))
            {
                return Err(problem);
            }

            step("Removing what the old scripts left", &mut done);
            let removed = recipe::run(&exec, &retire::steps())
                .await
                .map_err(step_error)?;
            info!(instance = %name, ?removed, "removed the old scripts' leftovers");

            step("Keeping git's name and email", &mut done);
            let cmd = ["sh", "-c", KEEP_GIT_SETTINGS, "sh", recipe::AGENT_HOME];
            super::exec::agent_ok(&exec, &cmd)
                .await
                .map_err(|error| format!("Git's name and email didn't carry over: {error}"))?;

            step("Moving the repos to the work folder", &mut done);
            let top = top_folder(&record.found).unwrap_or_default();
            let cmd = ["sh", "-c", MOVE_REPOS, "sh", recipe::AGENT_HOME, &top];
            let moved = exec
                .run(&cmd, ExecUser::Agent, None)
                .await
                .map_err(|e| e.to_string())?;
            if !moved.success() {
                return Err(format!(
                    "The repos didn't move to the work folder: {}",
                    String::from_utf8_lossy(&moved.stderr).trim()
                ));
            }

            for recipe_step in &steps {
                step(&format!("Recipe step {}", recipe_step.id), &mut done);
                recipe::ensure(&exec, recipe_step)
                    .await
                    .map_err(step_error)?;
            }

            step("Waiting for the guest", &mut done);
            let guest = manager.link_guest(workspace, &name).await?;
            manager.set_guest(workspace, guest);
            if let Some(record) = manager.record(workspace) {
                manager.apply_author(&record).await?;
            }

            let Some(mut keys) = manager.record(workspace).map(|r| r.keys) else {
                return Err("its record is gone".to_owned());
            };
            keys.recipe = manager.config().recipe;
            incus
                .set_config(&name, keys.encode())
                .await
                .map_err(|e| e.to_string())?;
            manager.ensure_project(&keys);
            let host_boot = manager.inner.host_boot.clone();
            manager.update(workspace, |record| {
                record.keys = keys;
                record.state = WorkspaceState::Running;
                record.host_boot = Some(host_boot);
            });
            Ok(())
        }
        .await;
        if let Err(error) = &result {
            warn!(instance = %name, %error, "a move's converge failed");
            manager.config().links.close(workspace);
            manager.set_state(
                workspace,
                WorkspaceState::Failed {
                    reason: format!("The converge failed: {error}"),
                },
            );
        }
        result
    }

    async fn drop_old_copy(&self, record: &Move) -> Result<(), String> {
        let manager = &self.manager;
        let name = Self::name(record)?;
        let incus = manager.incus();
        let Some(found) = incus.instance(&name).await.map_err(|e| e.to_string())? else {
            return Ok(());
        };
        let workspace = Keys::decode(&found.config).ok().map(|keys| keys.workspace);
        if let Some(workspace) = workspace {
            self.sessions.stop_in(workspace).await?;
            manager.config().links.close(workspace);
        }
        if found.status != "Stopped" {
            incus
                .set_state(
                    &name,
                    StateAction::Stop,
                    manager.config().timing.state_timeout,
                )
                .await
                .map_err(|e| e.to_string())?;
        }
        incus.delete(&name).await.map_err(|e| e.to_string())?;
        manager.remove_socket(&name);
        if let Some(workspace) = workspace {
            manager.forget(workspace);
        }
        Ok(())
    }

    async fn resume_agent(&self, resume: Resume) -> Result<(), String> {
        let manager = &self.manager;
        if !resume
            .session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err("The session's ID isn't one a move can resume.".to_owned());
        }
        let account = self.account(resume.workspace, resume.vendor)?;
        let Some(record) = manager.record(resume.workspace) else {
            return Err("The copy's workspace is gone.".to_owned());
        };
        let name = InstanceName::new(&record.instance).map_err(|e| e.to_string())?;
        let (from, to) = match resume.vendor {
            Vendor::Claude => (
                "/home/agent/.claude".to_owned(),
                recipe::accounts::claude_folder(&account.to_string()),
            ),
            _ => (
                "/home/agent/.codex".to_owned(),
                recipe::accounts::codex_folder(&account.to_string()),
            ),
        };
        let exec = IncusExec::new(manager.incus().clone(), name);
        let to = to.to_string();
        let project = project_folder(&resume.cwd);
        let cmd = [
            "sh",
            "-c",
            CARRY_OVER,
            "sh",
            recipe::AGENT_HOME,
            &from,
            &to,
            &resume.session,
            &project,
        ];
        let output = exec
            .run(&cmd, ExecUser::Agent, None)
            .await
            .map_err(|e| e.to_string())?;
        if !output.success() {
            return Err(format!(
                "The session's history didn't copy: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        self.sessions
            .resume_moved(ResumeMoved {
                workspace: resume.workspace,
                account,
                vendor: resume.vendor,
                session: resume.session,
                cwd: resume.cwd,
            })
            .await
    }

    /// The account a moved agent runs under: the project's default when it
    /// fits, or the only one of that vendor on this device.
    fn account(&self, workspace: WorkspaceId, vendor: Vendor) -> Result<AccountId, String> {
        let hub = &self.manager.config().hub;
        let device = hub.device().id;
        let accounts: Vec<AccountId> = hub
            .snapshot()
            .entities
            .into_iter()
            .filter_map(|entity| match entity {
                Entity::Account(login) if login.device == device && login.vendor == vendor => {
                    Some(login.id)
                }
                _ => None,
            })
            .collect();
        if let Some(project) = record_project(&self.manager, workspace)
            && let Some(Entity::Project(project)) = hub.get(&EntityKey::Project(project))
            && let Some(account) = project.default_account
            && accounts.contains(&account)
        {
            return Ok(account);
        }
        match accounts.as_slice() {
            [account] => Ok(*account),
            [] => Err(format!(
                "There's no {} account on this device to resume the session with.",
                vendor_name(vendor)
            )),
            _ => Err(format!(
                "There's more than one {} account on this device. Pick one as the \
                 project's default, then switch again.",
                vendor_name(vendor)
            )),
        }
    }
}

fn record_project(manager: &Manager, workspace: WorkspaceId) -> Option<ProjectId> {
    manager.record(workspace).map(|record| record.keys.project)
}

impl Host for MoveHost {
    fn copy_setup<'a>(&'a self, record: &'a Move) -> BoxFuture<'a, Result<CopySetup, String>> {
        Box::pin(self.setup(record))
    }

    fn converge<'a>(&'a self, record: &'a Move) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(self.converge_copy(record))
    }

    fn drop_copy<'a>(&'a self, record: &'a Move) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(self.drop_old_copy(record))
    }

    fn runs_with(&self, vendor: Vendor) -> String {
        let hub = &self.manager.config().hub;
        let device = hub.device();
        let settings = match hub.get(&EntityKey::DeviceSettings(device.id)) {
            Some(Entity::DeviceSettings(settings)) => settings,
            _ => crate::hub::default_settings(device),
        };
        let mode = match settings.default_permission {
            PermissionMode::Ask => "Ask",
            PermissionMode::AcceptEdits => "Accept edits",
            PermissionMode::Auto => "Auto",
            PermissionMode::Plan => "Plan",
            PermissionMode::FullAccess => "Full access",
        };
        let labels: Vec<String> = hub
            .snapshot()
            .entities
            .into_iter()
            .filter_map(|entity| match entity {
                Entity::Account(login) if login.device == device.id && login.vendor == vendor => {
                    Some(login.label)
                }
                _ => None,
            })
            .collect();
        let account = match labels.as_slice() {
            [] => "no account yet".to_owned(),
            [label] => format!("the account {label}"),
            _ => "the project's default account".to_owned(),
        };
        format!("in {mode} mode, on {account}")
    }

    fn resume(&self, resume: Resume) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(self.resume_agent(resume))
    }

    fn stop_agents(&self, workspace: WorkspaceId) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(self.sessions.stop_in(workspace))
    }

    fn unsaved(&self, workspace: WorkspaceId) -> BoxFuture<'_, Vec<String>> {
        Box::pin(async move {
            let record = self.manager.record(workspace);
            match record.as_ref().and_then(|record| record.unsaved.as_ref()) {
                Some(summary) => unsaved_parts(summary),
                None => vec![format!(
                    "maxd hasn't heard from {} about unsaved work yet",
                    record.map_or_else(|| "the copy".to_owned(), |record| record.instance)
                )],
            }
        })
    }
}

/// The device's moves, over its workspace manager and sessions.
pub fn mover(manager: Manager, sessions: Sessions, db: crate::db::Db) -> crate::moves::Mover {
    let config = manager.config();
    crate::moves::Mover::open(crate::moves::Config {
        incus: manager.incus().clone(),
        hub: config.hub.clone(),
        db,
        device: config.hub.device().id,
        resume_flags: crate::moves::RESUME_FLAGS.into(),
        uptime: "/proc/uptime".into(),
        state_timeout: config.timing.state_timeout,
        now: std::sync::Arc::new(Timestamp::now),
        host: std::sync::Arc::new(MoveHost {
            manager: manager.clone(),
            sessions,
        }),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::{Command, Output};

    use super::*;
    use crate::testing::TempDir;

    const SESSION: &str = "11111111-2222-3333-4444-555555555555";

    fn sh(script: &str, args: &[&str]) -> Output {
        Command::new("sh")
            .args(["-c", script, "sh"])
            .args(args)
            .output()
            .unwrap()
    }

    /// The pre-commit hook runs these tests with its own repo in GIT_DIR, and
    /// git would act on that repo instead of the test's folder.
    fn without_git_env(command: &mut Command) -> &mut Command {
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_") {
                command.env_remove(name);
            }
        }
        command
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let mut command = Command::new("git");
        let output = without_git_env(&mut command)
            .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn moved(home: &Path, top: &str) -> Output {
        sh(MOVE_REPOS, &[home.to_str().unwrap(), top])
    }

    fn keep_git_settings(home: &Path, system: &Path) -> Output {
        let mut command = Command::new("sh");
        without_git_env(&mut command)
            .env("GIT_CONFIG_SYSTEM", system)
            .args(["-c", KEEP_GIT_SETTINGS, "sh", home.to_str().unwrap()])
            .output()
            .unwrap()
    }

    #[test]
    fn git_name_and_email_move_to_the_home_config() {
        let dir = TempDir::new();
        let system = dir.path().join("system");
        write(
            &system,
            "[user]\n\tname = Sam\n\temail = sam@example.com\n\
             [init]\n\tdefaultBranch = main\n\
             [safe]\n\tdirectory = *\n\
             [credential]\n\thelper = old\n",
        );

        let output = keep_git_settings(dir.path(), &system);
        assert!(output.status.success(), "{output:?}");

        let config = dir.path().join(".gitconfig");
        let get = |key: &str| {
            git(
                dir.path(),
                &["config", "--file", config.to_str().unwrap(), "--get", key],
            )
        };
        assert_eq!(get("user.name"), "Sam");
        assert_eq!(get("user.email"), "sam@example.com");
        assert_eq!(get("init.defaultBranch"), "main");
        let text = fs::read_to_string(&config).unwrap();
        assert!(!text.contains("safe"), "{text}");
        assert!(!text.contains("helper"), "{text}");
    }

    #[test]
    fn a_home_git_setting_stands() {
        let dir = TempDir::new();
        let system = dir.path().join("system");
        write(&system, "[user]\n\tname = Old\n\temail = old@example.com\n");
        let config = dir.path().join(".gitconfig");
        write(&config, "[user]\n\tname = Mine\n");

        let output = keep_git_settings(dir.path(), &system);
        assert!(output.status.success(), "{output:?}");

        let get = |key: &str| {
            git(
                dir.path(),
                &["config", "--file", config.to_str().unwrap(), "--get", key],
            )
        };
        assert_eq!(get("user.name"), "Mine");
        assert_eq!(get("user.email"), "old@example.com");
    }

    #[test]
    fn nothing_to_keep_after_the_recipe_replaced_the_system_config() {
        let dir = TempDir::new();
        let system = dir.path().join("system");
        write(&system, "[credential]\n\thelper = new\n");

        let replaced = keep_git_settings(dir.path(), &system);
        assert!(replaced.status.success(), "{replaced:?}");
        let missing = keep_git_settings(dir.path(), &dir.path().join("missing"));
        assert!(missing.status.success(), "{missing:?}");

        assert!(!dir.path().join(".gitconfig").exists());
    }

    #[test]
    fn repos_go_in_the_work_folder_and_their_old_paths_still_work() {
        let dir = TempDir::new();
        let home = dir.path();
        let repo = home.join("widgets");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let worktree = home.join("widgets-fix");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "fix",
                worktree.to_str().unwrap(),
            ],
        );
        write(&home.join("notes/todo.txt"), "stays");
        write(&home.join("scratch/run.sh"), "named by the session");

        let output = moved(home, "scratch");
        assert!(output.status.success(), "{output:?}");

        for name in ["widgets", "widgets-fix", "scratch"] {
            let old = home.join(name);
            assert_eq!(
                fs::read_link(&old).unwrap(),
                Path::new("work").join(name),
                "{name}"
            );
            assert!(home.join("work").join(name).is_dir(), "{name}");
        }
        assert!(fs::symlink_metadata(home.join("notes")).unwrap().is_dir());

        // Git still finds a worktree by the absolute paths it wrote before.
        let new_worktree = home.join("work/widgets-fix");
        assert_eq!(git(&new_worktree, &["branch", "--show-current"]), "fix");
        assert_eq!(git(&worktree, &["branch", "--show-current"]), "fix");
        let listed = git(&home.join("work/widgets"), &["worktree", "list"]);
        assert_eq!(listed.lines().count(), 2, "{listed}");
        git(&home.join("work/widgets"), &["worktree", "prune"]);
        assert_eq!(git(&new_worktree, &["branch", "--show-current"]), "fix");

        // A second run finds the links and leaves them.
        let output = moved(home, "scratch");
        assert!(output.status.success(), "{output:?}");
        assert!(home.join("work/widgets/.git").is_dir());
        assert_eq!(
            fs::read_link(home.join("widgets")).unwrap(),
            Path::new("work/widgets")
        );
    }

    #[test]
    fn a_repo_the_work_folder_has_already_stays_where_it_is() {
        let dir = TempDir::new();
        let home = dir.path();
        write(&home.join("widgets/.git/HEAD"), "mine");
        write(&home.join("work/widgets/.git/HEAD"), "already there");

        let output = moved(home, "");
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("already there"));
        assert_eq!(
            fs::read_to_string(home.join("widgets/.git/HEAD")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn a_home_with_no_repos_and_a_session_in_work_moves_nothing() {
        let dir = TempDir::new();
        let home = dir.path();
        write(&home.join("notes/todo.txt"), "stays");
        write(&home.join("work/widgets/.git/HEAD"), "there");

        let output = moved(home, "work");
        assert!(output.status.success(), "{output:?}");
        assert!(fs::symlink_metadata(home.join("work")).unwrap().is_dir());
        assert!(fs::symlink_metadata(home.join("notes")).unwrap().is_dir());
    }

    /// The home folder the scripts see, with `widgets` moved and `other` not.
    fn carried(prepare: impl Fn(&Path, &Path)) -> (TempDir, String) {
        let dir = TempDir::new();
        let home = dir.path();
        let text = home.to_str().unwrap().to_owned();
        let from = home.join(".claude");
        fs::create_dir(home.join("work")).unwrap();
        std::os::unix::fs::symlink("work/widgets", home.join("widgets")).unwrap();
        fs::create_dir(home.join("other")).unwrap();
        let to = home.join(".claude-account");
        prepare(&from, &to);
        let project = project_folder(&format!("{text}/work/widgets"));
        let output = sh(
            CARRY_OVER,
            &[
                &text,
                from.to_str().unwrap(),
                to.to_str().unwrap(),
                SESSION,
                &project,
            ],
        );
        assert!(output.status.success(), "{output:?}");
        (dir, text)
    }

    #[test]
    fn the_switch_carries_what_the_cli_reads_from_its_old_folder() {
        let (dir, text) = carried(|from, _| {
            let home = from.parent().unwrap().to_str().unwrap();
            let old = project_folder(&format!("{home}/widgets"));
            let sub = project_folder(&format!("{home}/widgets/packages"));
            let other = project_folder(&format!("{home}/other"));
            write(&from.join("CLAUDE.md"), "my rules");
            write(&from.join("settings.json"), "{}");
            let projects = from.join("projects");
            write(
                &projects.join(&old).join(format!("{SESSION}.jsonl")),
                "chat",
            );
            write(&projects.join(&old).join("older.jsonl"), "old chat");
            write(&projects.join(&old).join("memory/MEMORY.md"), "widgets");
            write(&projects.join(&sub).join("memory/MEMORY.md"), "packages");
            write(&projects.join(&other).join("memory/MEMORY.md"), "other");
        });
        let to = dir.path().join(".claude-account");
        let read = |path: &Path| fs::read_to_string(to.join(path)).unwrap();
        let projects = Path::new("projects");
        assert_eq!(read(Path::new("CLAUDE.md")), "my rules");
        assert_eq!(read(Path::new("settings.json")), "{}");

        let new = project_folder(&format!("{text}/work/widgets"));
        assert_eq!(
            read(&projects.join(&new).join(format!("{SESSION}.jsonl"))),
            "chat"
        );
        assert_eq!(
            read(&projects.join(&new).join("memory/MEMORY.md")),
            "widgets"
        );
        assert!(!to.join(projects).join(&new).join("older.jsonl").exists());

        let sub = project_folder(&format!("{text}/work/widgets/packages"));
        assert_eq!(
            read(&projects.join(sub).join("memory/MEMORY.md")),
            "packages"
        );
        let other = project_folder(&format!("{text}/other"));
        assert_eq!(
            read(&projects.join(other).join("memory/MEMORY.md")),
            "other"
        );
    }

    #[test]
    fn the_switch_leaves_what_the_accounts_folder_has_already() {
        let (dir, text) = carried(|from, to| {
            let home = from.parent().unwrap().to_str().unwrap();
            let old = project_folder(&format!("{home}/widgets"));
            let new = project_folder(&format!("{home}/work/widgets"));
            write(&from.join("settings.json"), "old");
            write(
                &from.join("projects").join(old).join("memory/MEMORY.md"),
                "old",
            );
            write(&to.join("settings.json"), "new");
            write(
                &to.join("projects").join(new).join("memory/MEMORY.md"),
                "new",
            );
        });
        let to = dir.path().join(".claude-account");
        let new = project_folder(&format!("{text}/work/widgets"));
        assert_eq!(fs::read_to_string(to.join("settings.json")).unwrap(), "new");
        assert_eq!(
            fs::read_to_string(to.join("projects").join(new).join("memory/MEMORY.md")).unwrap(),
            "new"
        );
    }

    #[test]
    fn a_codex_session_keeps_its_dated_folder() {
        let dated = format!("sessions/2026/11/06/rollout-1-{SESSION}.jsonl");
        let (dir, _) = carried(|from, _| write(&from.join(&dated), "chat"));
        assert_eq!(
            fs::read_to_string(dir.path().join(".claude-account").join(&dated)).unwrap(),
            "chat"
        );
    }

    #[test]
    fn a_cli_that_was_never_used_has_nothing_to_carry() {
        let (dir, _) = carried(|_, _| {});
        assert!(!dir.path().join(".claude-account").exists());
    }
}
