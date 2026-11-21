// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::collections::{HashMap, VecDeque};
use std::fs;

use maxhq_core::skills::disk::DiskFs;
use maxhq_core::skills::tree;
use maxhq_protocol::guest::ScannedFile;
use maxhq_protocol::ids::{BootId, DeviceId};
use maxhq_protocol::model::Device;
use maxhq_protocol::skills::SkillProposal;

use super::*;
use crate::incus::fake::{FakeIncus, State as IncusState};
use crate::skills::apply::tests::stocked;
use crate::testing::TempDir;

fn hub() -> Hub {
    Hub::new(
        Device {
            id: DeviceId::derive("devbox"),
            name: "devbox".into(),
            os: "linux".into(),
            boot_id: BootId("b".into()),
            maxd_version: "0.1.0".into(),
            winding_down_until: None,
            workspace_host: None,
            build: None,
            target: None,
        },
        HashMap::new(),
    )
}

/// Records what it was told.
#[derive(Default)]
struct Told(Mutex<Vec<Changed>>);

impl Running for Told {
    fn skills_changed(&self, changed: Vec<Changed>) {
        self.0.lock().unwrap().extend(changed);
    }
}

/// A guest that records each link request, makes nothing, and says a
/// pack installed.
#[derive(Default)]
struct FakeGuests(Mutex<Vec<GuestLinks>>, Mutex<VecDeque<FolderScan>>);

impl GuestSkills for FakeGuests {
    fn link_skills(
        &self,
        _: WorkspaceId,
        links: GuestLinks,
    ) -> BoxFuture<Result<Option<PackState>, String>> {
        let pack = links.pack.clone().map(|tree| PackState::Installed { tree });
        self.0.lock().unwrap().push(links);
        Box::pin(async { Ok(pack) })
    }

    fn folder_scan(
        &self,
        _: WorkspaceId,
        _: String,
        _: Vec<String>,
    ) -> BoxFuture<Result<FolderScan, String>> {
        let scan = self.1.lock().unwrap().pop_front();
        Box::pin(async move { scan.ok_or_else(|| "no scan".to_owned()) })
    }
}

/// A link request without a pack.
fn linked(checkout: &str, homes: Vec<String>) -> GuestLinks {
    GuestLinks {
        checkout: checkout.into(),
        names: vec!["git-flow".into()],
        always: vec!["unslop".into()],
        homes,
        pack: None,
        retry: true,
    }
}

struct Setup {
    dir: TempDir,
    hub: Hub,
    project: ProjectId,
    skills: SkillLock,
    delivery: Arc<Delivery>,
    told: Arc<Told>,
    guests: Arc<FakeGuests>,
}

/// A delivery whose store has `git-flow`, with `unslop` in the always set.
fn setup(incus: Option<Incus>) -> Setup {
    let dir = TempDir::new();
    let (store, mut skills) = stocked(dir.path(), &[("git-flow", "one"), ("unslop", "two")]);
    let always = skills.skills.split_off(1);
    let hub = hub();
    let guests = Arc::new(FakeGuests::default());
    let delivery = Arc::new(Delivery::open(
        hub.clone(),
        Db::open_in_memory().unwrap(),
        Arc::new(store),
        incus,
        dir.path().join("sets"),
        Arc::new(move || always.clone()),
        Some(guests.clone()),
    ));
    let told = Arc::new(Told::default());
    let _ = delivery.running.set(told.clone());
    Setup {
        dir,
        hub,
        project: ProjectId::generate(),
        skills,
        delivery,
        told,
        guests,
    }
}

impl Setup {
    /// A workspace of the project at `location`.
    fn workspace(&self, location: WorkspaceLocation) -> WorkspaceId {
        let id = WorkspaceId::generate();
        self.hub.apply([Change::Upsert {
            value: Entity::Workspace(Workspace {
                id,
                project: Some(self.project),
                device: self.hub.device().id,
                location,
                managed: None,
                clone: None,
            }),
        }]);
        id
    }

    /// A folder checkout of the project.
    fn folder(&self, name: &str) -> (WorkspaceId, PathBuf) {
        let path = self.dir.path().join(name);
        fs::create_dir_all(path.join(".git/info")).unwrap();
        let id = self.workspace(WorkspaceLocation::Folder {
            path: path.display().to_string(),
        });
        (id, path)
    }

    fn published(&self) -> ProjectSkills {
        let key = EntityKey::ProjectSkills {
            project: self.project,
            device: self.hub.device().id,
        };
        match self.hub.get(&key) {
            Some(Entity::ProjectSkills(skills)) => skills,
            other => panic!("expected the project's skills, got {other:?}"),
        }
    }

    fn proposals(&self) -> Vec<SkillProposal> {
        self.hub.pick(|entity| match entity {
            Entity::SkillProposal(proposal) => Some(proposal.clone()),
            _ => None,
        })
    }
}

fn place(workspace: WorkspaceId, cwd: &Path) -> Place {
    Place {
        device: DeviceId::derive("devbox"),
        workspace,
        cwd: cwd.to_owned(),
        attachments: cwd.join(".attachments"),
        host: PlaceHost::Local,
        dispatcher: false,
        trust: None,
        claude_artifacts: true,
    }
}

#[tokio::test]
async fn an_apply_puts_the_lock_in_each_folder_checkout_and_says_so() {
    let setup = setup(None);
    let (_, repo) = setup.folder("repo");
    setup
        .delivery
        .set_project(setup.project, setup.skills.clone())
        .unwrap();
    assert!(setup.published().checkouts.is_empty());

    setup.delivery.clone().apply(setup.project).await.unwrap();
    assert!(repo.join(".maxhq/skills/git-flow/SKILL.md").is_file());
    let published = setup.published();
    assert_eq!(published.lock, setup.skills);
    let [checkout] = published.checkouts.as_slice() else {
        panic!("expected one checkout, got {:?}", published.checkouts);
    };
    assert_eq!(checkout.path, repo.display().to_string());
    assert!(checkout.applied && checkout.drift.is_empty() && !checkout.needs_restart);
    assert_eq!(
        *setup.told.0.lock().unwrap(),
        [Changed {
            guest: None,
            path: repo.clone()
        }]
    );

    // Nothing changed, so nobody is told again.
    setup.delivery.clone().apply(setup.project).await.unwrap();
    assert_eq!(setup.told.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_checkout_says_which_link_stopped_its_skills() {
    let setup = setup(None);
    let (_, repo) = setup.folder("repo");
    let outside = setup.dir.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let empty = || fs::read_dir(&outside).unwrap().count() == 0;
    std::os::unix::fs::symlink(&outside, repo.join(".claude")).unwrap();
    setup
        .delivery
        .set_project(setup.project, setup.skills.clone())
        .unwrap();

    // The other CLI's links and the copies land without it.
    setup.delivery.clone().apply(setup.project).await.unwrap();
    let published = setup.published();
    assert_eq!(published.checkouts[0].outside, [".claude"]);
    assert!(published.checkouts[0].applied);
    assert!(repo.join(".maxhq/skills/git-flow/SKILL.md").is_file());
    assert!(repo
        .join(".agents/skills/git-flow")
        .symlink_metadata()
        .is_ok());
    assert!(empty());

    // Once the link goes, the next apply lands there too.
    fs::remove_file(repo.join(".claude")).unwrap();
    setup.delivery.clone().apply(setup.project).await.unwrap();
    assert!(setup.published().checkouts[0].outside.is_empty());
    assert!(repo
        .join(".claude/skills/git-flow")
        .symlink_metadata()
        .is_ok());

    // A link at MaxHQ's own folder stops all of it.
    let (_, second) = setup.folder("second");
    std::os::unix::fs::symlink(&outside, second.join(".maxhq")).unwrap();
    setup.delivery.clone().apply(setup.project).await.unwrap();
    let published = setup.published();
    let stopped = published
        .checkouts
        .iter()
        .find(|checkout| Path::new(&checkout.path) == second)
        .unwrap();
    assert_eq!(stopped.outside, [".maxhq"]);
    assert!(!stopped.applied);
    assert!(empty());
}

#[tokio::test]
async fn a_lock_survives_a_restart_of_maxd() {
    let setup = setup(None);
    let db = setup.delivery.db.clone();
    setup
        .delivery
        .set_project(setup.project, setup.skills.clone())
        .unwrap();
    let hub = hub();
    let again = Delivery::open(
        hub.clone(),
        db,
        setup.delivery.store.clone(),
        None,
        setup.dir.path().join("sets"),
        Arc::new(Vec::new),
        None,
    );
    assert_eq!(again.lock().locks[&setup.project], setup.skills);
    let key = EntityKey::ProjectSkills {
        project: setup.project,
        device: hub.device().id,
    };
    assert!(hub.get(&key).is_some());
}

#[tokio::test]
async fn an_apply_waits_for_the_trees_and_a_lock_keeps_off_the_always_names() {
    let setup = setup(None);
    let err = setup
        .delivery
        .clone()
        .apply(setup.project)
        .await
        .unwrap_err();
    assert!(err.contains("no skills"), "{err}");

    let mut skills = setup.skills.clone();
    skills.skills[0].tree = maxhq_protocol::skills::TreeHash("c".repeat(64));
    setup.delivery.set_project(setup.project, skills).unwrap();
    let err = setup
        .delivery
        .clone()
        .apply(setup.project)
        .await
        .unwrap_err();
    assert!(err.contains("doesn't have every skill"), "{err}");

    let mut clash = setup.skills.clone();
    clash.skills[0].name = "unslop".into();
    assert!(setup.delivery.set_project(setup.project, clash).is_err());
}

#[test]
fn a_launch_restores_a_changed_copy_and_keeps_it_as_a_proposal() {
    let setup = setup(None);
    let (workspace, repo) = setup.folder("repo");
    let place = place(workspace, &repo);
    // A project with no lock gets nothing in its checkouts.
    assert_eq!(setup.delivery.ensure(&place), Ok(Ensured::default()));
    assert!(!repo.join(".maxhq").exists());

    setup
        .delivery
        .set_project(setup.project, setup.skills.clone())
        .unwrap();
    assert!(setup.delivery.ensure(&place).unwrap().changed);
    let copy = repo.join(".maxhq/skills/git-flow/SKILL.md");
    let mut modes = fs::metadata(&copy).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut modes, 0o644);
    fs::set_permissions(&copy, modes).unwrap();
    fs::write(&copy, "---\nname: git-flow\ndescription: changed\n---\nnew").unwrap();

    let ensured = setup.delivery.ensure(&place).unwrap();
    assert_eq!(ensured.drift, ["git-flow"]);
    assert_eq!(
        tree::of(&DiskFs, &repo.join(".maxhq/skills/git-flow")).unwrap(),
        setup.skills.skills[0].tree
    );
    let [proposal] = setup.proposals().try_into().unwrap();
    assert_eq!(proposal.name, "git-flow");
    assert_eq!(proposal.project, setup.project);
    assert_eq!(proposal.base, Some(setup.skills.skills[0].tree.clone()));
    assert_eq!(proposal.source, Some(setup.skills.skills[0].source));
    assert_eq!(proposal.checkout, repo.display().to_string());
    assert_eq!(setup.published().checkouts[0].drift, ["git-flow"]);
}

#[test]
fn a_restart_mark_shows_until_the_agents_restart_and_outlasts_an_apply() {
    let setup = setup(None);
    let (workspace, repo) = setup.folder("repo");
    setup
        .delivery
        .set_project(setup.project, setup.skills.clone())
        .unwrap();
    setup.delivery.ensure(&place(workspace, &repo)).unwrap();
    setup.delivery.restart_marked(&repo, true);
    assert!(setup.published().checkouts[0].needs_restart);
    setup.delivery.ensure(&place(workspace, &repo)).unwrap();
    assert!(setup.published().checkouts[0].needs_restart);
    setup.delivery.restart_marked(&repo, false);
    assert!(!setup.published().checkouts[0].needs_restart);
}

#[tokio::test]
async fn a_container_gets_its_set_on_a_read_only_disk_once_and_links_to_it() {
    let mut state = IncusState::default();
    state.add(
        "mhq-lantern",
        "Running",
        &[(crate::incus::MANAGED_KEY, "true")],
    );
    let fake = FakeIncus::start(state);
    let setup = setup(Some(Incus::new(&fake.socket)));
    let workspace = setup.workspace(WorkspaceLocation::Container {
        instance: "mhq-lantern".into(),
        path: "/work/repo".into(),
    });
    setup
        .delivery
        .set_project(setup.project, setup.skills.clone())
        .unwrap();

    setup.delivery.clone().apply(setup.project).await.unwrap();
    let root = setup.dir.path().join("sets").join(workspace.to_string());
    assert!(root.join("current/git-flow/SKILL.md").is_file());
    assert!(root.join("current/unslop/SKILL.md").is_file());
    let devices = fake.state().instances["mhq-lantern"]["devices"].clone();
    assert_eq!(devices["maxhq-skills"]["readonly"], "true");
    assert_eq!(devices["maxhq-skills"]["path"], "/opt/maxhq/skills");
    assert_eq!(
        devices["maxhq-skills"]["source"],
        root.display().to_string().as_str()
    );
    assert_eq!(
        *setup.guests.0.lock().unwrap(),
        [linked("/work/repo", Vec::new())]
    );
    assert_eq!(
        *setup.told.0.lock().unwrap(),
        [Changed {
            guest: Some(workspace),
            path: "/work/repo".into()
        }]
    );

    // A new worktree gets linked with its homes, and the disk isn't added
    // again.
    let homes = vec!["/home/agent/.agents/skills".to_owned()];
    setup
        .delivery
        .clone()
        .ensure_guest(workspace, "/work/repo/.worktrees/a".into(), homes.clone())
        .await
        .unwrap();
    let patches = fake
        .state()
        .writes()
        .iter()
        .filter(|write| write.method == "PATCH")
        .count();
    assert_eq!(patches, 1);
    let found = setup.guests.0.lock().unwrap().clone();
    let worktree = GuestLinks {
        retry: false,
        ..linked("/work/repo/.worktrees/a", homes)
    };
    assert!(found.contains(&worktree));
    let paths: Vec<String> = setup
        .published()
        .checkouts
        .into_iter()
        .map(|checkout| checkout.path)
        .collect();
    assert_eq!(paths, ["/work/repo", "/work/repo/.worktrees/a"]);
}

#[tokio::test]
async fn a_gone_workspace_loses_its_set_and_the_others_keep_theirs() {
    let mut state = IncusState::default();
    for instance in ["mhq-lantern", "mhq-harbor"] {
        state.add(instance, "Running", &[(crate::incus::MANAGED_KEY, "true")]);
    }
    let fake = FakeIncus::start(state);
    let setup = setup(Some(Incus::new(&fake.socket)));
    let [gone, kept] = ["mhq-lantern", "mhq-harbor"].map(|instance| {
        setup.workspace(WorkspaceLocation::Container {
            instance: instance.into(),
            path: "/work/repo".into(),
        })
    });
    setup
        .delivery
        .set_project(setup.project, setup.skills.clone())
        .unwrap();
    setup.delivery.clone().apply(setup.project).await.unwrap();
    let root = |workspace: WorkspaceId| setup.dir.path().join("sets").join(workspace.to_string());
    assert!(root(gone).join("current/git-flow/SKILL.md").is_file());

    setup.delivery.forget_workspace(gone);
    assert!(!root(gone).exists());
    assert!(!setup.delivery.lock().guest_homes.contains_key(&gone));
    assert!(root(kept).join("current/git-flow/SKILL.md").is_file());
    assert!(setup.delivery.lock().guest_homes.contains_key(&kept));
}

/// The setup's lock with a fake pack whose installer notes each run in
/// the checkout, and `exit` after.
fn with_pack(setup: &Setup, exit: u8) -> SkillLock {
    let from = setup.dir.path().join(format!("pack-{exit}"));
    fs::create_dir_all(&from).unwrap();
    let script = from.join("install.sh");
    let body = format!("#!/usr/bin/env bash\necho ran >> installed.txt\nexit {exit}\n");
    fs::write(&script, body).unwrap();
    let mode = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    fs::set_permissions(&script, mode).unwrap();
    let tree = tree::of(&DiskFs, &from).unwrap();
    setup.delivery.store.land(&tree, &from).unwrap();
    let mut skills = setup.skills.clone();
    skills.pack = Some(maxhq_protocol::skills::SkillPack {
        source: skills.skills[0].source,
        commit: "abc".into(),
        tree,
    });
    skills
}

fn runs(repo: &Path) -> usize {
    fs::read_to_string(repo.join("installed.txt"))
        .unwrap_or_default()
        .lines()
        .count()
}

#[tokio::test]
async fn an_apply_installs_the_pack_once_and_a_removal_takes_the_copy_out() {
    let setup = setup(None);
    let (_, repo) = setup.folder("repo");
    let skills = with_pack(&setup, 0);
    let tree = skills.pack.clone().unwrap().tree;
    setup.delivery.set_project(setup.project, skills).unwrap();
    setup.delivery.clone().apply(setup.project).await.unwrap();
    setup.delivery.clone().apply(setup.project).await.unwrap();
    assert_eq!(runs(&repo), 1);
    assert_eq!(
        setup.published().checkouts[0].pack,
        Some(PackState::Installed { tree })
    );
    let exclude = fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
    assert!(exclude.contains("/.devflow-vendor/") && exclude.contains("/CLAUDE.md"));
    let logs = setup.dir.path().join(pack::LOGS);
    assert!(pack::log_path(&logs, &repo).is_file());

    setup
        .delivery
        .set_project(setup.project, setup.skills.clone())
        .unwrap();
    setup.delivery.clone().apply(setup.project).await.unwrap();
    assert!(!repo.join(".devflow-vendor").exists());
    assert!(repo.join("installed.txt").is_file());
    assert_eq!(setup.published().checkouts[0].pack, None);
}

#[tokio::test]
async fn a_failed_install_waits_for_the_next_apply_not_a_launch() {
    let setup = setup(None);
    let (workspace, repo) = setup.folder("repo");
    setup
        .delivery
        .set_project(setup.project, with_pack(&setup, 3))
        .unwrap();
    setup.delivery.ensure(&place(workspace, &repo)).unwrap();
    setup.delivery.ensure(&place(workspace, &repo)).unwrap();
    assert_eq!(runs(&repo), 1);
    let Some(PackState::Failed { lines, .. }) = setup.published().checkouts[0].pack.clone() else {
        panic!("the install should fail");
    };
    assert!(lines.last().unwrap().contains("stopped with"));
    setup.delivery.clone().apply(setup.project).await.unwrap();
    assert_eq!(runs(&repo), 2);
}

#[tokio::test]
async fn someone_elses_pack_is_marked_and_left_alone() {
    let setup = setup(None);
    let (_, repo) = setup.folder("repo");
    fs::create_dir_all(repo.join(".devflow-vendor")).unwrap();
    setup
        .delivery
        .set_project(setup.project, with_pack(&setup, 0))
        .unwrap();
    setup.delivery.clone().apply(setup.project).await.unwrap();
    assert_eq!(runs(&repo), 0);
    assert_eq!(
        setup.published().checkouts[0].pack,
        Some(PackState::Vendored)
    );
    let exclude = fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
    assert!(!exclude.contains("/.devflow-vendor/"));
}

#[tokio::test]
async fn a_container_gets_its_pack_through_the_set_and_its_guest() {
    let setup = setup(None);
    let workspace = setup.workspace(WorkspaceLocation::Container {
        instance: "mhq-lantern".into(),
        path: "/work/repo".into(),
    });
    let skills = with_pack(&setup, 0);
    let tree = skills.pack.clone().unwrap().tree;
    setup.delivery.set_project(setup.project, skills).unwrap();
    setup.delivery.clone().apply(setup.project).await.unwrap();
    let root = setup.dir.path().join("sets").join(workspace.to_string());
    assert!(root.join("current/.devflow-vendor/install.sh").is_file());
    let found = setup.guests.0.lock().unwrap().clone();
    assert_eq!(found[0].pack, Some(tree.clone()));
    assert!(found[0].retry);
    assert_eq!(
        setup.published().checkouts[0].pack,
        Some(PackState::Installed { tree })
    );
}

#[tokio::test]
async fn a_container_install_keeps_trust_only_if_what_was_there_was_fine() {
    let file = |name: &str| ScannedFile {
        name: name.into(),
        digest: "abc".into(),
    };
    let scan = |files: Vec<ScannedFile>| FolderScan {
        key: "/work/repo".into(),
        files,
        ..FolderScan::default()
    };
    let setup = setup(None);
    let workspace = setup.workspace(WorkspaceLocation::Container {
        instance: "mhq-lantern".into(),
        path: "/work/repo".into(),
    });
    let key = format!("guest:{workspace}:/work/repo");
    let skills = with_pack(&setup, 0);
    setup.delivery.set_project(setup.project, skills).unwrap();

    // Hostile files were there already, so the install blesses nothing.
    let hostile = vec![file(".claude/settings.json"), file(".claude/hooks/pack.sh")];
    *setup.guests.1.lock().unwrap() = VecDeque::from([
        scan(vec![file(".claude/settings.json")]),
        scan(hostile.clone()),
    ]);
    setup.delivery.clone().apply(setup.project).await.unwrap();
    let db = &setup.delivery.db;
    assert!(!db
        .folder_trusted(&key, &folder_trust::fingerprint(&hostile))
        .unwrap());

    // An empty folder stays fine once the install lands.
    let landed = vec![file(".claude/hooks/pack.sh")];
    *setup.guests.1.lock().unwrap() = VecDeque::from([scan(Vec::new()), scan(landed.clone())]);
    setup.delivery.clone().apply(setup.project).await.unwrap();
    assert!(db
        .folder_trusted(&key, &folder_trust::fingerprint(&landed))
        .unwrap());
}

#[test]
fn a_container_agent_reads_the_shared_folder_and_claude_its_config_too() {
    let config = Path::new("/home/agent/.claude-1");
    assert_eq!(
        guest_homes(Vendor::Claude, config),
        ["/home/agent/.agents/skills", "/home/agent/.claude-1/skills"]
    );
    assert_eq!(
        guest_homes(Vendor::Codex, config),
        ["/home/agent/.agents/skills"]
    );
}
