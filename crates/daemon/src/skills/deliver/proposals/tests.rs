// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use maxhq_protocol::guest::GuestProposalFile;
use maxhq_protocol::ids::{BootId, DeviceId};
use maxhq_protocol::model::{Device, DeviceSettings, Workspace};
use maxhq_protocol::skills::{LockedSkill, PackState, SkillInfo, SourceKind, TreeHash};

use super::*;
use crate::db::Db;
use crate::hub::Hub;
use crate::session::clock::{BoxFuture, FakeClock};
use crate::skills::deliver::{GuestLinks, GuestSkills};
use crate::skills::library::NewSource;
use crate::skills::sources::{self, tests::repo};
use crate::skills::store::SkillStore;
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

/// A container's guest with proposals to hand back, which records what
/// it was asked to remove.
#[derive(Default)]
struct FakeGuests {
    proposals: Mutex<Vec<GuestProposal>>,
    removed: Mutex<Vec<String>>,
}

impl GuestSkills for FakeGuests {
    fn link_skills(
        &self,
        _: WorkspaceId,
        _: GuestLinks,
    ) -> BoxFuture<Result<Option<PackState>, String>> {
        Box::pin(async { Ok(None) })
    }

    fn read_proposals(
        &self,
        _: WorkspaceId,
        _: String,
        remove: Option<String>,
    ) -> BoxFuture<Result<Vec<GuestProposal>, String>> {
        let mut proposals = self.proposals.lock().unwrap();
        if let Some(folder) = remove {
            proposals.retain(|proposal| proposal.folder != folder);
            self.removed.lock().unwrap().push(folder);
        }
        let read = proposals.clone();
        Box::pin(async move { Ok(read) })
    }
}

struct Rig {
    dir: TempDir,
    hub: Hub,
    project: ProjectId,
    library: Library,
    delivery: Arc<Delivery>,
    guests: Arc<FakeGuests>,
}

impl Rig {
    /// A device that holds the library, whose store the delivery shares.
    fn new() -> Self {
        let dir = TempDir::new();
        let hub = hub();
        hub.apply([Change::Upsert {
            value: Entity::DeviceSettings(DeviceSettings {
                skill_library: Some(true),
                ..crate::hub::default_settings(hub.device())
            }),
        }]);
        let data = dir.path().join("data");
        let store = Arc::new(SkillStore::under(&data));
        let db = Db::open_in_memory().unwrap();
        let clock = Arc::new(FakeClock::new());
        let library = Library::open(hub.clone(), db.clone(), store.clone(), &data, clock);
        let guests = Arc::new(FakeGuests::default());
        let delivery = Arc::new(Delivery::open(
            hub.clone(),
            db,
            store,
            None,
            data.join("skills/workspaces"),
            Arc::new(Vec::new),
            Some(guests.clone()),
        ));
        Self {
            dir,
            hub,
            project: ProjectId::generate(),
            library,
            delivery,
            guests,
        }
    }

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
    fn checkout(&self) -> (PathBuf, Checkout) {
        let path = self.dir.path().join("repo");
        fs::create_dir_all(path.join(".git/info")).unwrap();
        self.workspace(WorkspaceLocation::Folder {
            path: path.display().to_string(),
        });
        let at = Checkout {
            guest: None,
            path: path.display().to_string(),
        };
        (path, at)
    }

    /// `git-flow` in the personal source, locked by the project.
    async fn locked(&self) -> SkillInfo {
        let folder = self.dir.path().join("made/git-flow");
        write(&folder, "git-flow", "One");
        let personal = self.library.personal().await.unwrap();
        let skill = self
            .library
            .commit_folder(personal, "git-flow", &folder, "Add git-flow")
            .await
            .unwrap();
        self.lock(&skill);
        skill
    }

    fn lock(&self, skill: &SkillInfo) {
        let lock = SkillLock {
            skills: vec![LockedSkill {
                name: skill.name.clone(),
                source: skill.source,
                path: skill.path.clone(),
                commit: None,
                tree: skill.tree.clone(),
                pinned: false,
            }],
            committed: false,
            pack: None,
        };
        self.delivery.set_project(self.project, lock).unwrap();
    }

    fn proposals(&self) -> Vec<SkillProposal> {
        self.hub.pick(|entity| match entity {
            Entity::SkillProposal(proposal) => Some(proposal.clone()),
            _ => None,
        })
    }

    fn personal_log(&self) -> String {
        let root = self.dir.path().join("data/skills/personal");
        sources::in_repo(&root, &["log", "--format=%s by %an"]).unwrap()
    }

    async fn answer(
        &self,
        id: SkillProposalId,
        action: ProposalAction,
        fork: Option<&str>,
    ) -> Result<(), String> {
        let answer = Answer {
            proposal: id,
            action,
            push: false,
            fork: fork.map(str::to_owned),
            elsewhere: None,
            seen: None,
        };
        self.delivery
            .clone()
            .answer(Some(&self.library), answer)
            .await
    }
}

impl Rig {
    /// Approves `id` as the owner who read `seen`.
    async fn approve_seen(&self, id: SkillProposalId, seen: ProposalSeen) -> Result<(), String> {
        let answer = Answer {
            proposal: id,
            action: ProposalAction::Approve,
            push: false,
            fork: None,
            elsewhere: None,
            seen: Some(seen),
        };
        self.delivery
            .clone()
            .answer(Some(&self.library), answer)
            .await
    }

    /// Approves `found`, from another device, as the owner who read `seen`.
    async fn approve_elsewhere(
        &self,
        found: &SkillProposal,
        seen: Option<ProposalSeen>,
    ) -> Result<(), String> {
        let answer = Answer {
            proposal: found.id,
            action: ProposalAction::Approve,
            push: false,
            fork: None,
            elsewhere: Some(Box::new(found.clone())),
            seen,
        };
        self.delivery
            .clone()
            .answer(Some(&self.library), answer)
            .await
    }

    /// What `id` changes, as this device that keeps the library shows it.
    async fn held_diff(&self, id: SkillProposalId) -> Result<ProposalDiff, String> {
        let sources = Some(self.library.sources());
        self.delivery.clone().diff(id, sources).await
    }

    /// The project locks nothing, so what it finds claims no base.
    fn unlock(&self) {
        let lock = SkillLock {
            skills: Vec::new(),
            committed: false,
            pack: None,
        };
        self.delivery.set_project(self.project, lock).unwrap();
    }

    /// A proposal from another device that says what it's told to, with
    /// the files at `folder` in this device's store.
    fn proposal_from_elsewhere(&self, folder: &Path, name: &str) -> SkillProposal {
        let tree = tree::of(&DiskFs, folder).unwrap();
        self.delivery.keep(&tree, folder).unwrap();
        SkillProposal {
            id: SkillProposalId::derive("elsewhere"),
            device: maxhq_protocol::ids::DeviceId::derive("other"),
            project: self.project,
            agent: None,
            name: name.to_owned(),
            base: None,
            tree,
            checkout: "/work/repo".into(),
            created_at: Timestamp(1),
            flags: SkillFlags::default(),
            source: None,
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.library.store().open_up();
    }
}

/// A skill folder at `folder` called `name`.
fn write(folder: &Path, name: &str, description: &str) {
    fs::create_dir_all(folder).unwrap();
    fs::write(
        folder.join(SKILL_FILE),
        format!("---\nname: {name}\ndescription: {description}\n---\nSteps.\n"),
    )
    .unwrap();
}

#[tokio::test]
async fn a_turn_end_finds_a_proposal_that_keeps_its_agent_and_goes_with_its_folder() {
    let rig = Rig::new();
    let skill = rig.locked().await;
    let (repo, at) = rig.checkout();
    let folder = repo.join(".maxhq/proposals/git-flow");
    write(&folder, "git-flow", "Two");
    fs::create_dir_all(repo.join(".maxhq/proposals/.hidden")).unwrap();
    let agent = AgentId::generate();

    rig.delivery.found(rig.project, &at, Some(agent));
    let [proposal] = rig.proposals().try_into().unwrap();
    assert_eq!(proposal.agent, Some(agent));
    assert_eq!(proposal.name, "git-flow");
    assert_eq!(proposal.base, Some(skill.tree.clone()));
    assert_eq!(proposal.source, Some(skill.source));
    assert_eq!(proposal.tree, tree::of(&DiskFs, &folder).unwrap());
    assert_eq!(proposal.checkout, repo.display().to_string());
    assert_eq!(proposal.device, rig.hub.device().id);

    // Found again with no agent, it's the same proposal.
    rig.delivery.found(rig.project, &at, None);
    assert_eq!(rig.proposals(), [proposal]);

    fs::remove_dir_all(&folder).unwrap();
    rig.delivery.found(rig.project, &at, None);
    assert!(rig.proposals().is_empty());
}

#[tokio::test]
async fn drift_and_a_scan_make_one_proposal_that_comes_back_after_a_restart() {
    let rig = Rig::new();
    rig.locked().await;
    let (repo, at) = rig.checkout();
    let place = crate::drive::Place {
        device: rig.hub.device().id,
        workspace: rig.workspace(WorkspaceLocation::Folder {
            path: repo.display().to_string(),
        }),
        cwd: repo.clone(),
        attachments: repo.join(".attachments"),
        host: crate::drive::PlaceHost::Local,
        dispatcher: false,
        trust: None,
        claude_artifacts: true,
    };
    rig.delivery.ensure(&place).unwrap();
    let copy = repo.join(".maxhq/skills/git-flow");
    fs::set_permissions(copy.join(SKILL_FILE), fs::Permissions::from_mode(0o644)).unwrap();
    write(&copy, "git-flow", "Changed in place");
    assert_eq!(rig.delivery.ensure(&place).unwrap().drift, ["git-flow"]);
    let [drift] = rig.proposals().try_into().unwrap();
    assert_eq!(drift.agent, None);

    rig.delivery.found(rig.project, &at, None);
    assert_eq!(rig.proposals(), std::slice::from_ref(&drift));

    // A new run finds it from the project's workspaces alone.
    rig.hub.apply([Change::Remove {
        key: EntityKey::SkillProposal(drift.id),
    }]);
    rig.delivery.lock().proposals.clear();
    rig.delivery.clone().look_all(rig.project).await;
    let [again] = rig.proposals().try_into().unwrap();
    assert_eq!((again.id, again.tree), (drift.id, drift.tree));
}

#[tokio::test]
async fn approving_commits_as_you_and_takes_the_folder_away() {
    let rig = Rig::new();
    rig.locked().await;
    let (repo, at) = rig.checkout();
    let folder = repo.join(".maxhq/proposals/git-flow");
    write(&folder, "git-flow", "Two");
    rig.delivery.found(rig.project, &at, None);
    let [proposal] = rig.proposals().try_into().unwrap();

    rig.answer(proposal.id, ProposalAction::Approve, None)
        .await
        .unwrap();
    assert_eq!(
        rig.personal_log().lines().collect::<Vec<_>>(),
        [
            "Take the proposed change to git-flow by Test",
            "Add git-flow by Test"
        ]
    );
    let [source] = rig.library.sources().try_into().unwrap();
    assert_eq!(source.skills[0].tree, proposal.tree);
    assert_eq!(source.skills[0].description, "Two");
    assert!(!folder.exists());
    assert!(rig.proposals().is_empty());

    let gone = rig
        .answer(proposal.id, ProposalAction::Approve, None)
        .await
        .unwrap_err();
    assert_eq!(gone, "This proposal isn't on this device any more.");
}

#[tokio::test]
async fn a_proposal_that_changed_since_it_was_found_isnt_approved() {
    let rig = Rig::new();
    rig.locked().await;
    let (repo, at) = rig.checkout();
    let folder = repo.join(".maxhq/proposals/git-flow");
    write(&folder, "git-flow", "Two");
    rig.delivery.found(rig.project, &at, None);
    let [proposal] = rig.proposals().try_into().unwrap();
    write(&folder, "git-flow", "Three");

    let refused = rig
        .answer(proposal.id, ProposalAction::Approve, None)
        .await
        .unwrap_err();
    assert!(refused.starts_with("The proposal changed"), "{refused}");
    assert_eq!(rig.personal_log().lines().count(), 1);
    let [now] = rig.proposals().try_into().unwrap();
    assert_eq!(now.tree, tree::of(&DiskFs, &folder).unwrap());
}

#[tokio::test]
async fn a_change_to_a_proposal_after_its_check_doesnt_reach_the_commit() {
    let rig = Rig::new();
    rig.locked().await;
    let (repo, at) = rig.checkout();
    let folder = repo.join(".maxhq/proposals/git-flow");
    write(&folder, "git-flow", "Two");
    rig.delivery.found(rig.project, &at, None);
    let [proposal] = rig.proposals().try_into().unwrap();

    // The library's lock holds the approval at the commit, past its check.
    let busy = rig.library.busy().await;
    let mirror = rig.delivery.mirror();
    let change = async {
        for _ in 0..200 {
            let copied = fs::read_dir(&mirror)
                .into_iter()
                .flatten()
                .flatten()
                .any(|at| {
                    fs::read_to_string(at.path().join(SKILL_FILE))
                        .is_ok_and(|text| text.ends_with("Steps.\n"))
                });
            if copied {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        write(&folder, "git-flow", "Three");
        drop(busy);
    };
    let (approved, ()) = tokio::join!(
        rig.answer(proposal.id, ProposalAction::Approve, None),
        change
    );
    approved.unwrap();

    let landed = rig
        .dir
        .path()
        .join("data/skills/personal/git-flow")
        .join(SKILL_FILE);
    let landed = fs::read_to_string(landed).unwrap();
    assert!(landed.contains("description: Two"), "{landed}");
}

#[tokio::test]
async fn a_third_partys_skill_comes_back_only_as_a_named_fork() {
    let rig = Rig::new();
    let remote = repo(&rig.dir.path().join("remote"), &[("alpha", "First")]);
    let theirs = rig
        .library
        .add(NewSource {
            name: "Theirs".into(),
            kind: SourceKind::Git {
                url: remote.display().to_string(),
            },
            default_ref: None,
            trust: SourceTrust::ThirdParty,
        })
        .await
        .unwrap();
    rig.lock(&theirs.skills[0]);
    let (repo, at) = rig.checkout();
    let folder = repo.join(".maxhq/proposals/alpha");
    write(&folder, "alpha", "Better");
    rig.delivery.found(rig.project, &at, None);
    let [proposal] = rig.proposals().try_into().unwrap();
    assert_eq!(proposal.source, Some(theirs.id));

    let refused = rig
        .answer(proposal.id, ProposalAction::Approve, None)
        .await
        .unwrap_err();
    assert!(refused.contains("Name a fork"), "{refused}");
    let bad = rig
        .answer(proposal.id, ProposalAction::Approve, Some("Not A Name"))
        .await
        .unwrap_err();
    assert!(bad.starts_with("The name Not A Name"), "{bad}");
    assert!(folder.exists());

    rig.answer(proposal.id, ProposalAction::Approve, Some("alpha-mine"))
        .await
        .unwrap();
    let personal = rig.library.personal().await.unwrap();
    let sources = rig.library.sources();
    let mine = sources.iter().find(|source| source.id == personal).unwrap();
    let [fork] = mine.skills.as_slice() else {
        panic!("expected the fork, got {:?}", mine.skills);
    };
    assert_eq!(
        (fork.name.as_str(), fork.description.as_str()),
        ("alpha-mine", "Better")
    );
    assert_eq!(
        rig.personal_log().trim(),
        "Fork alpha as alpha-mine by Test"
    );
    let theirs_now = sources
        .iter()
        .find(|source| source.id == theirs.id)
        .unwrap();
    assert_eq!(theirs_now.skills, theirs.skills);
    assert!(!folder.exists());

    // The same name twice is refused.
    write(&folder, "alpha", "Again");
    rig.delivery.found(rig.project, &at, None);
    let [again] = rig.proposals().try_into().unwrap();
    let taken = rig
        .answer(again.id, ProposalAction::Approve, Some("alpha-mine"))
        .await
        .unwrap_err();
    assert!(
        taken.contains("already has a skill called alpha-mine"),
        "{taken}"
    );
}

#[tokio::test]
async fn rejecting_removes_the_folder_and_the_proposal() {
    let rig = Rig::new();
    let (repo, at) = rig.checkout();
    let folder = repo.join(".maxhq/proposals/brand-new");
    write(&folder, "brand-new", "New");
    rig.delivery.found(rig.project, &at, None);
    let [proposal] = rig.proposals().try_into().unwrap();
    assert_eq!((proposal.base.clone(), proposal.source), (None, None));

    rig.answer(proposal.id, ProposalAction::Reject, None)
        .await
        .unwrap();
    assert!(!folder.exists());
    assert!(rig.proposals().is_empty());
}

#[tokio::test]
async fn a_proposals_folder_that_became_a_link_isnt_read_or_emptied() {
    for link in [".maxhq", ".maxhq/proposals"] {
        let rig = Rig::new();
        let (repo, at) = rig.checkout();
        write(&repo.join(".maxhq/proposals/mine"), "mine", "Mine");
        rig.delivery.found(rig.project, &at, None);
        let [proposal] = rig.proposals().try_into().unwrap();

        // The same folder, now reached through a link to outside the checkout.
        let outside = rig.dir.path().join("outside");
        fs::rename(repo.join(link), &outside).unwrap();
        std::os::unix::fs::symlink(&outside, repo.join(link)).unwrap();
        let kept = if link == ".maxhq" {
            outside.join("proposals/mine/SKILL.md")
        } else {
            outside.join("mine/SKILL.md")
        };

        let refused = rig
            .answer(proposal.id, ProposalAction::Reject, None)
            .await
            .unwrap_err();
        assert!(refused.contains("is a link"), "{link}: {refused}");
        assert!(kept.is_file(), "{link}");
        let refused = rig
            .delivery
            .clone()
            .diff(proposal.id, None)
            .await
            .unwrap_err();
        assert!(refused.contains("is a link"), "{link}: {refused}");

        // The listing drops it, so it isn't shown to act on either.
        rig.delivery.found(rig.project, &at, None);
        assert!(rig.proposals().is_empty(), "{link}");
        assert!(kept.is_file(), "{link}");
    }
}

#[tokio::test]
async fn a_proposals_link_to_the_folder_above_doesnt_offer_a_sibling_checkout() {
    let rig = Rig::new();
    let (repo, at) = rig.checkout();
    let sibling = rig.dir.path().join("sibling");
    write(&sibling, "sibling", "Someone else's work");
    fs::create_dir_all(sibling.join(".git")).unwrap();
    fs::write(sibling.join(".git/HEAD"), "unpushed").unwrap();
    fs::create_dir_all(repo.join(".maxhq")).unwrap();
    std::os::unix::fs::symlink("../..", repo.join(".maxhq/proposals")).unwrap();

    rig.delivery.found(rig.project, &at, None);

    assert!(rig.proposals().is_empty());
    assert!(sibling.join(".git/HEAD").is_file());
}

#[tokio::test]
async fn a_pipe_in_a_proposal_doesnt_hold_up_the_look() {
    let rig = Rig::new();
    let (repo, at) = rig.checkout();
    let folder = repo.join(".maxhq/proposals/mine");
    write(&folder, "mine", "Mine");
    let made = std::process::Command::new("mkfifo")
        .arg(folder.join("notes"))
        .status();
    assert!(made.unwrap().success());

    let (delivery, project) = (rig.delivery.clone(), rig.project);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        delivery.found(project, &at, None);
        let _ = tx.send(());
    });
    rx.recv_timeout(std::time::Duration::from_secs(10))
        .expect("the look is still waiting on the pipe");
}

#[tokio::test]
async fn the_diff_gives_each_changed_file_against_the_base() {
    let rig = Rig::new();
    rig.locked().await;
    let (repo, at) = rig.checkout();
    let folder = repo.join(".maxhq/proposals/git-flow");
    write(&folder, "git-flow", "Two");
    fs::create_dir_all(folder.join("scripts")).unwrap();
    fs::write(folder.join("scripts/run.sh"), "echo hi\n").unwrap();
    fs::set_permissions(
        folder.join("scripts/run.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    rig.delivery.found(rig.project, &at, None);
    let [proposal] = rig.proposals().try_into().unwrap();

    let diff = rig.held_diff(proposal.id).await.unwrap();
    assert!(diff.flags.scripts);
    let [skill, script] = diff.files.as_slice() else {
        panic!("expected two files, got {:?}", diff.files);
    };
    assert_eq!(
        (skill.path.as_str(), skill.status),
        ("SKILL.md", PullFileStatus::Modified)
    );
    assert_eq!((skill.additions, skill.deletions), (1, 1));
    let patch = skill.patch.as_deref().unwrap();
    assert!(patch.starts_with("@@"), "{patch}");
    assert!(
        patch.contains("-description: One\n+description: Two"),
        "{patch}"
    );
    assert_eq!(
        (script.path.as_str(), script.status, script.additions),
        ("scripts/run.sh", PullFileStatus::Added, 1)
    );
    // The diff keeps the proposal's tree, ready to copy to the holder.
    assert!(rig.delivery.store.has(&proposal.tree));
}

#[tokio::test]
async fn a_containers_proposals_come_through_its_guest() {
    let rig = Rig::new();
    let workspace = rig.workspace(WorkspaceLocation::Container {
        instance: "ws-example".into(),
        path: "/work/example".into(),
    });
    let data = |text: &str| STANDARD.encode(text);
    rig.guests.proposals.lock().unwrap().push(GuestProposal {
        folder: "brand-new".into(),
        files: vec![
            GuestProposalFile {
                path: SKILL_FILE.into(),
                executable: false,
                data: data("---\nname: brand-new\ndescription: New\n---\n"),
            },
            GuestProposalFile {
                path: "scripts/go.sh".into(),
                executable: true,
                data: data("echo go\n"),
            },
        ],
    });
    rig.delivery.clone().look_all(rig.project).await;
    let [proposal] = rig.proposals().try_into().unwrap();
    assert_eq!(proposal.checkout, "/work/example");
    assert!(proposal.flags.scripts);
    let at = Checkout {
        guest: Some(workspace),
        path: "/work/example".into(),
    };
    let mirrored = rig
        .delivery
        .proposals_dir(&at)
        .unwrap()
        .join("brand-new/scripts/go.sh");
    let mode = fs::metadata(&mirrored).unwrap().permissions().mode();
    assert_eq!(mode & 0o111, 0o111);

    rig.answer(proposal.id, ProposalAction::Reject, None)
        .await
        .unwrap();
    assert_eq!(*rig.guests.removed.lock().unwrap(), ["brand-new"]);
    assert!(rig.proposals().is_empty());
    assert!(!mirrored.exists());
}

#[test]
fn a_mirror_takes_only_plain_paths() {
    let dir = TempDir::new();
    let bad = GuestProposal {
        folder: "ok".into(),
        files: vec![GuestProposalFile {
            path: "../escape".into(),
            executable: false,
            data: String::new(),
        }],
    };
    assert!(mirror(&dir.path().join("m"), &[bad]).is_err());
    assert!(!dir.path().join("escape").exists());
}

#[test]
fn a_fork_renames_the_skill_in_its_frontmatter() {
    assert_eq!(
        with_name("---\nname: alpha\ndescription: A\n---\nBody\n", "beta"),
        "---\nname: beta\ndescription: A\n---\nBody\n"
    );
    assert_eq!(
        with_name("---\ndescription: A\n---\n", "beta"),
        "---\nname: beta\ndescription: A\n---\n"
    );
    assert_eq!(with_name("Body", "beta"), "---\nname: beta\n---\nBody");
}

#[tokio::test]
async fn a_proposal_with_no_base_says_when_it_replaces_a_skill() {
    let rig = Rig::new();
    let skill = rig.locked().await;
    rig.unlock();
    let (repo, at) = rig.checkout();
    let folder = repo.join(".maxhq/proposals/git-flow");
    write(&folder, "git-flow", "Two");
    rig.delivery.found(rig.project, &at, None);
    let [proposal] = rig.proposals().try_into().unwrap();
    assert_eq!(proposal.base, None);

    let diff = rig.held_diff(proposal.id).await.unwrap();
    let target = diff.target.clone().unwrap();
    assert_eq!(target.base, Some(skill.tree.clone()));
    assert_eq!(target.source, Some(skill.source));
    assert_eq!(diff.files[0].status, PullFileStatus::Modified);

    // An app from before this showed it as new, so it's refused.
    let refused = rig
        .answer(proposal.id, ProposalAction::Approve, None)
        .await
        .unwrap_err();
    assert!(refused.contains("already has a skill"), "{refused}");
    assert!(folder.exists());
    let [source] = rig.library.sources().try_into().unwrap();
    assert_eq!(source.skills[0].tree, skill.tree);

    rig.approve_seen(
        proposal.id,
        ProposalSeen {
            tree: proposal.tree.clone(),
            base: target.base,
        },
    )
    .await
    .unwrap();
    let [source] = rig.library.sources().try_into().unwrap();
    assert_eq!(source.skills[0].tree, proposal.tree);
}

#[tokio::test]
async fn an_approve_must_match_what_the_owner_read() {
    let rig = Rig::new();
    let skill = rig.locked().await;
    let (repo, at) = rig.checkout();
    let folder = repo.join(".maxhq/proposals/git-flow");
    write(&folder, "git-flow", "Two");
    rig.delivery.found(rig.project, &at, None);
    let [proposal] = rig.proposals().try_into().unwrap();
    let elsewhere = TreeHash("elsewhere".into());

    for seen in [
        ProposalSeen {
            tree: elsewhere.clone(),
            base: Some(skill.tree.clone()),
        },
        ProposalSeen {
            tree: proposal.tree.clone(),
            base: Some(elsewhere.clone()),
        },
        ProposalSeen {
            tree: proposal.tree.clone(),
            base: None,
        },
    ] {
        let refused = rig.approve_seen(proposal.id, seen).await.unwrap_err();
        assert_eq!(refused, CHANGED);
    }
    assert!(folder.exists());
    let [source] = rig.library.sources().try_into().unwrap();
    assert_eq!(source.skills[0].tree, skill.tree);

    rig.approve_seen(
        proposal.id,
        ProposalSeen {
            tree: proposal.tree.clone(),
            base: Some(skill.tree.clone()),
        },
    )
    .await
    .unwrap();
    let [source] = rig.library.sources().try_into().unwrap();
    assert_eq!(source.skills[0].tree, proposal.tree);
}

#[tokio::test]
async fn the_library_moving_after_the_diff_stops_the_approve() {
    let rig = Rig::new();
    let skill = rig.locked().await;
    let (repo, at) = rig.checkout();
    let folder = repo.join(".maxhq/proposals/git-flow");
    write(&folder, "git-flow", "Two");
    rig.delivery.found(rig.project, &at, None);
    let [proposal] = rig.proposals().try_into().unwrap();
    let diff = rig.held_diff(proposal.id).await.unwrap();
    let read = diff.target.unwrap().base;
    assert_eq!(read, Some(skill.tree));

    let newer = rig.dir.path().join("made-again/git-flow");
    write(&newer, "git-flow", "Three");
    let personal = rig.library.personal().await.unwrap();
    rig.library
        .commit_folder(personal, "git-flow", &newer, "Move on")
        .await
        .unwrap();

    let seen = ProposalSeen {
        tree: proposal.tree,
        base: read,
    };
    let refused = rig.approve_seen(proposal.id, seen).await.unwrap_err();
    assert_eq!(refused, CHANGED);
    assert!(folder.exists());
}

#[tokio::test]
async fn a_proposal_from_another_device_is_judged_by_its_tree_and_the_library() {
    let rig = Rig::new();
    let skill = rig.locked().await;
    let made = rig.dir.path().join("elsewhere/git-flow");
    write(&made, "git-flow", "Hostile");
    fs::create_dir_all(made.join("scripts")).unwrap();
    fs::write(made.join("scripts/run.sh"), "echo hi\n").unwrap();

    // It says it adds a harmless skill, but its files are for git-flow.
    let lying = rig.proposal_from_elsewhere(&made, "harmless");
    let sources = Some(rig.library.sources());
    let refused = rig
        .delivery
        .clone()
        .diff_elsewhere(lying.clone(), sources)
        .await
        .unwrap_err();
    assert!(refused.contains("another skill"), "{refused}");
    let seen = ProposalSeen {
        tree: lying.tree.clone(),
        base: None,
    };
    let refused = rig.approve_elsewhere(&lying, Some(seen)).await.unwrap_err();
    assert!(refused.contains("another skill"), "{refused}");

    // Named right, with no flags and a made-up base, the diff still compares
    // with the library's skill and reads the flags from the tree.
    let claims = SkillProposal {
        base: Some(TreeHash("made-up".into())),
        ..rig.proposal_from_elsewhere(&made, "git-flow")
    };
    let sources = Some(rig.library.sources());
    let diff = rig
        .delivery
        .clone()
        .diff_elsewhere(claims.clone(), sources)
        .await
        .unwrap();
    assert_eq!(diff.tree.as_ref(), Some(&claims.tree));
    assert!(diff.flags.scripts);
    let target = diff.target.unwrap();
    assert_eq!(target.base, Some(skill.tree.clone()));
    assert_eq!(target.source, Some(skill.source));
    assert_eq!(diff.files[0].path, "SKILL.md");
    assert_eq!(diff.files[0].status, PullFileStatus::Modified);

    // With no base claimed and an app from before, it can't replace it.
    let unclaimed = rig.proposal_from_elsewhere(&made, "git-flow");
    let refused = rig.approve_elsewhere(&unclaimed, None).await.unwrap_err();
    assert!(refused.contains("already has a skill"), "{refused}");
    let [source] = rig.library.sources().try_into().unwrap();
    assert_eq!(source.skills[0].tree, skill.tree);

    // Approved against what the owner read, it lands where it was shown.
    let seen = ProposalSeen {
        tree: claims.tree.clone(),
        base: target.base,
    };
    rig.approve_elsewhere(&claims, Some(seen)).await.unwrap();
    let [source] = rig.library.sources().try_into().unwrap();
    assert_eq!(source.skills[0].tree, claims.tree);
}

#[tokio::test]
async fn a_name_a_third_party_holds_asks_for_a_fork_even_when_the_proposal_doesnt_say() {
    let rig = Rig::new();
    let remote = repo(&rig.dir.path().join("remote"), &[("alpha", "First")]);
    let theirs = rig
        .library
        .add(NewSource {
            name: "Theirs".into(),
            kind: SourceKind::Git {
                url: remote.display().to_string(),
            },
            default_ref: None,
            trust: SourceTrust::ThirdParty,
        })
        .await
        .unwrap();
    let made = rig.dir.path().join("elsewhere/alpha");
    write(&made, "alpha", "Hostile");
    let found = rig.proposal_from_elsewhere(&made, "alpha");
    let sources = Some(rig.library.sources());
    let diff = rig
        .delivery
        .clone()
        .diff_elsewhere(found.clone(), sources)
        .await
        .unwrap();
    let target = diff.target.unwrap();
    assert_eq!(target.source, Some(theirs.id));
    assert_eq!(target.base, Some(theirs.skills[0].tree.clone()));
    let seen = ProposalSeen {
        tree: found.tree.clone(),
        base: target.base,
    };
    let refused = rig.approve_elsewhere(&found, Some(seen)).await.unwrap_err();
    assert!(refused.contains("Name a fork"), "{refused}");
}

#[tokio::test]
async fn the_diff_shows_the_tree_it_kept_not_the_folder_as_it_is_now() {
    let rig = Rig::new();
    rig.locked().await;
    let (repo, at) = rig.checkout();
    let folder = repo.join(".maxhq/proposals/git-flow");
    write(&folder, "git-flow", "Read by the owner");
    rig.delivery.found(rig.project, &at, None);
    let [proposal] = rig.proposals().try_into().unwrap();
    rig.held_diff(proposal.id).await.unwrap();

    // Changed in place, the kept copy still answers for this tree.
    write(&folder, "git-flow", "Swapped after");
    let diff = rig.held_diff(proposal.id).await.unwrap();
    assert_eq!(diff.tree.as_ref(), Some(&proposal.tree));
    let patch = diff.files[0].patch.as_deref().unwrap();
    assert!(patch.contains("Read by the owner"), "{patch}");
    assert!(!patch.contains("Swapped after"), "{patch}");
}

#[tokio::test]
async fn a_diff_of_a_folder_that_changed_before_it_was_kept_finds_it_again() {
    let rig = Rig::new();
    rig.locked().await;
    let (repo, at) = rig.checkout();
    let folder = repo.join(".maxhq/proposals/git-flow");
    write(&folder, "git-flow", "Two");
    rig.delivery.found(rig.project, &at, None);
    let [proposal] = rig.proposals().try_into().unwrap();

    write(&folder, "git-flow", "Three");
    assert!(rig.held_diff(proposal.id).await.is_err());
    let [again] = rig.proposals().try_into().unwrap();
    assert_eq!(again.id, proposal.id);
    assert_eq!(again.tree, tree::of(&DiskFs, &folder).unwrap());

    let diff = rig.held_diff(again.id).await.unwrap();
    assert_eq!(diff.tree, Some(again.tree));
}

#[tokio::test]
async fn a_skill_file_with_a_nul_is_neither_shown_nor_approved() {
    let rig = Rig::new();
    let skill = rig.locked().await;
    let (repo, at) = rig.checkout();
    let folder = repo.join(".maxhq/proposals/git-flow");
    write(&folder, "git-flow", "Two");
    let text = fs::read_to_string(folder.join(SKILL_FILE)).unwrap();
    fs::write(folder.join(SKILL_FILE), format!("{text}\0Hidden steps.\n")).unwrap();
    rig.delivery.found(rig.project, &at, None);
    let [proposal] = rig.proposals().try_into().unwrap();

    let refused = rig.held_diff(proposal.id).await.unwrap_err();
    assert!(refused.contains("NUL"), "{refused}");
    let refused = rig
        .answer(proposal.id, ProposalAction::Approve, None)
        .await
        .unwrap_err();
    assert!(refused.contains("NUL"), "{refused}");
    let [source] = rig.library.sources().try_into().unwrap();
    assert_eq!(source.skills[0].tree, skill.tree);

    rig.answer(proposal.id, ProposalAction::Reject, None)
        .await
        .unwrap();
    assert!(!folder.exists());
}
