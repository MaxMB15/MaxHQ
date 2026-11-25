// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::collections::HashMap;
use std::time::{Duration, UNIX_EPOCH};

use maxhq_core::skills::store::TreeStore;
use maxhq_protocol::ids::{BootId, DeviceId};
use maxhq_protocol::model::Device;
use rusqlite::params;

use super::*;
use crate::session::clock::FakeClock;
use crate::skills::sources::tests::{commit_all, repo, write_skills};
use crate::skills::store::KEEP_FOR;
use crate::testing::TempDir;

fn hub_on(os: &str) -> Hub {
    Hub::new(
        Device {
            id: DeviceId::derive(os),
            name: os.into(),
            os: os.into(),
            boot_id: BootId("b".into()),
            maxd_version: "0.1.0".into(),
            winding_down_until: None,
            workspace_host: None,
            build: None,
            target: None,
            wsl: None,
        },
        HashMap::new(),
    )
}

struct Rig {
    dir: TempDir,
    hub: Hub,
    db: Db,
    clock: Arc<FakeClock>,
    library: Library,
}

impl Rig {
    fn on(os: &str) -> Self {
        let dir = TempDir::new();
        let hub = hub_on(os);
        let db = Db::open_in_memory().unwrap();
        let clock = Arc::new(FakeClock::new());
        let library = Self::open(&dir, &hub, &db, &clock);
        Self {
            dir,
            hub,
            db,
            clock,
            library,
        }
    }

    fn open(dir: &TempDir, hub: &Hub, db: &Db, clock: &Arc<FakeClock>) -> Library {
        let data = dir.path().join("data");
        let store = Arc::new(SkillStore::under(&data));
        Library::open(hub.clone(), db.clone(), store, &data, clock.clone())
    }

    fn published(&self) -> Option<SkillLibrary> {
        match self.hub.get(&EntityKey::SkillLibrary(self.hub.device().id)) {
            Some(Entity::SkillLibrary(library)) => Some(library),
            _ => None,
        }
    }

    fn hold(&self, holds: Option<bool>) {
        self.hub.apply([Change::Upsert {
            value: Entity::DeviceSettings(DeviceSettings {
                skill_library: holds,
                ..crate::hub::default_settings(self.hub.device())
            }),
        }]);
    }

    fn git(&self, url: &Path, trust: SourceTrust) -> NewSource {
        NewSource {
            name: "Skills".into(),
            kind: SourceKind::Git {
                url: url.to_string_lossy().into_owned(),
            },
            default_ref: None,
            trust,
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.library.store().open_up();
    }
}

#[test]
fn a_mac_holds_the_library_unless_told_otherwise() {
    let mac = Rig::on("macos");
    assert!(mac.library.holds());
    mac.library.publish();
    let published = mac.published().unwrap();
    assert_eq!(published.device, mac.hub.device().id);
    assert_eq!(published.always, ["unslop", "propose-skill-change"]);

    mac.hold(Some(false));
    assert!(!mac.library.holds());
    mac.library.publish();
    assert_eq!(mac.published(), None);

    let linux = Rig::on("linux");
    assert!(!linux.library.holds());
    linux.hold(Some(true));
    assert!(linux.library.holds());
}

#[tokio::test]
async fn a_device_that_doesnt_hold_it_refuses_changes() {
    let rig = Rig::on("linux");
    let remote = repo(&rig.dir.path().join("remote"), &[("alpha", "First")]);
    let refused = rig
        .library
        .add(rig.git(&remote, SourceTrust::Own))
        .await
        .unwrap_err();
    assert_eq!(refused, NOT_HOLDER);
}

#[tokio::test]
async fn a_git_source_is_cloned_read_kept_and_published() {
    let rig = Rig::on("macos");
    let remote = repo(
        &rig.dir.path().join("remote"),
        &[("alpha", "First"), ("beta", "Second")],
    );
    let added = rig
        .library
        .add(rig.git(&remote, SourceTrust::ThirdParty))
        .await
        .unwrap();
    assert_eq!(added.skills.len(), 2);
    assert_eq!(added.commit, Some(sources::head(&remote).unwrap()));
    assert_eq!(added.fetched_at, Some(rig.clock.wall()));
    assert!(added
        .skills
        .iter()
        .all(|skill| rig.library.store().has(&skill.tree)));
    assert_eq!(rig.published().unwrap().sources, vec![added.clone()]);

    // Adding it again gives it as it is.
    let again = rig
        .library
        .add(rig.git(&remote, SourceTrust::ThirdParty))
        .await
        .unwrap();
    assert_eq!(again, added);

    // Reopened, the library has what it stored.
    let reopened = Rig::open(&rig.dir, &rig.hub, &rig.db, &rig.clock);
    assert_eq!(reopened.sources(), vec![added]);
}

#[tokio::test]
async fn a_source_that_cant_be_cloned_isnt_added() {
    let rig = Rig::on("macos");
    let missing = rig.dir.path().join("nowhere");
    let failed = rig
        .library
        .add(rig.git(&missing, SourceTrust::Own))
        .await
        .unwrap_err();
    assert!(failed.starts_with("git said:"), "{failed}");
    assert!(rig.library.sources().is_empty());
    assert!(rig.db.skill_sources().unwrap().is_empty());
}

#[tokio::test]
async fn a_fetch_follows_the_remote_and_a_failed_one_says_why() {
    let rig = Rig::on("macos");
    let remote = repo(&rig.dir.path().join("remote"), &[("alpha", "First")]);
    let added = rig
        .library
        .add(rig.git(&remote, SourceTrust::Own))
        .await
        .unwrap();
    let old = added.skills[0].tree.clone();

    write_skills(&remote, &[("alpha", "Changed")]);
    commit_all(&remote, "change");
    let fetched = rig.library.fetch(added.id).await.unwrap();
    assert_eq!(fetched.commit, Some(sources::head(&remote).unwrap()));
    assert_eq!(fetched.skills[0].description, "Changed");
    assert_ne!(fetched.skills[0].tree, old);
    assert_eq!(fetched.problem, None);

    fs::remove_dir_all(&remote).unwrap();
    let failed = rig.library.fetch(added.id).await.unwrap_err();
    let kept = rig.library.sources().remove(0);
    assert_eq!(kept.problem, Some(failed));
    assert_eq!(kept.skills, fetched.skills);
    assert_eq!(rig.published().unwrap().sources[0].problem, kept.problem);
}

#[tokio::test]
async fn removing_a_source_drops_its_clone_but_never_a_local_folder() {
    let rig = Rig::on("macos");
    let remote = repo(&rig.dir.path().join("remote"), &[("alpha", "First")]);
    let git = rig
        .library
        .add(rig.git(&remote, SourceTrust::Own))
        .await
        .unwrap();
    let clone = rig.library.clone_dir(git.id);
    assert!(clone.is_dir());
    let local = rig
        .library
        .add(NewSource {
            name: "Folder".into(),
            kind: SourceKind::Local {
                path: remote.to_string_lossy().into_owned(),
            },
            default_ref: None,
            trust: SourceTrust::Own,
        })
        .await
        .unwrap();
    assert_eq!(local.skills.len(), 1);

    rig.library.remove(git.id).await.unwrap();
    rig.library.remove(local.id).await.unwrap();
    assert!(!clone.exists());
    assert!(remote.join("skills/alpha/SKILL.md").is_file());
    assert!(rig.library.sources().is_empty());
    assert!(rig.db.skill_sources().unwrap().is_empty());
    assert_eq!(rig.published().unwrap().sources, []);
    // Removing it again is fine.
    rig.library.remove(git.id).await.unwrap();
}

#[tokio::test]
async fn a_committed_folder_lands_in_the_source_and_its_repo() {
    let rig = Rig::on("macos");
    let personal = rig.library.personal().await.unwrap();
    assert_eq!(rig.library.personal().await.unwrap(), personal);
    let folder = rig.dir.path().join("new/mine");
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join(SKILL_FILE),
        "---\nname: mine\ndescription: One\n---\n",
    )
    .unwrap();

    let skill = rig
        .library
        .commit_folder(personal, "mine", &folder, "Add mine")
        .await
        .unwrap();
    assert_eq!((skill.name.as_str(), skill.path.as_str()), ("mine", "mine"));
    assert!(rig.library.store().has(&skill.tree));
    assert!(folder.join(SKILL_FILE).is_file());
    let root = rig.dir.path().join("data/skills/personal");
    let log = sources::in_repo(&root, &["log", "--format=%s"]).unwrap();
    assert_eq!(log.trim(), "Add mine");

    // A changed copy replaces it in place, and the same one adds nothing.
    fs::write(
        folder.join(SKILL_FILE),
        "---\nname: mine\ndescription: Two\n---\n",
    )
    .unwrap();
    let changed = rig
        .library
        .commit_folder(personal, "mine", &folder, "Change mine")
        .await
        .unwrap();
    assert_eq!(changed.description, "Two");
    rig.library
        .commit_folder(personal, "mine", &folder, "Nothing")
        .await
        .unwrap();
    let log = sources::in_repo(&root, &["log", "--format=%s"]).unwrap();
    assert_eq!(log.lines().collect::<Vec<_>>(), ["Change mine", "Add mine"]);
    let source = rig.library.sources().remove(0);
    assert_eq!(source.skills, vec![changed]);
}

#[tokio::test]
async fn a_commit_lands_what_was_hashed_and_leaves_it_editable() {
    let rig = Rig::on("macos");
    let personal = rig.library.personal().await.unwrap();
    let folder = rig.dir.path().join("new/mine");
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join(SKILL_FILE),
        "---\nname: mine\ndescription: One\n---\n",
    )
    .unwrap();
    let skill = rig
        .library
        .commit_folder(personal, "mine", &folder, "Add mine")
        .await
        .unwrap();

    // What lands comes from the read that was hashed, not from the store's
    // copy, so a changed store copy doesn't reach the source.
    let kept = rig
        .library
        .store()
        .path(&skill.tree)
        .unwrap()
        .join(SKILL_FILE);
    // Windows has no modes to seal the store with.
    assert!(cfg!(windows) || fs::metadata(&kept).unwrap().permissions().readonly());
    maxhq_os::fsmode::set_mode(&kept, 0o644).unwrap();
    fs::write(&kept, "---\nname: mine\ndescription: Kept\n---\n").unwrap();
    rig.library
        .commit_folder(personal, "mine", &folder, "Again")
        .await
        .unwrap();

    let landed = rig
        .dir
        .path()
        .join("data/skills/personal/mine")
        .join(SKILL_FILE);
    let text = fs::read_to_string(&landed).unwrap();
    assert!(text.contains("One") && !text.contains("Kept"), "{text}");
    assert!(!fs::metadata(&landed).unwrap().permissions().readonly());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tidy_takes_a_global_skill_into_the_personal_source() {
    use crate::skills::globals::Library as Tidy;

    let rig = Rig::on("macos");
    let folder = rig.dir.path().join("global/unslop");
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join(SKILL_FILE),
        "---\nname: unslop\ndescription: Plain words\n---\n",
    )
    .unwrap();
    assert_eq!(Tidy::always_skill(&rig.library, "unslop"), None);

    let library = rig.library.clone();
    let tree =
        tokio::task::spawn_blocking(move || Tidy::commit_folder(&library, "unslop", &folder))
            .await
            .unwrap()
            .unwrap();
    assert!(rig.library.store().has(&tree));
    let skill = Tidy::always_skill(&rig.library, "unslop").unwrap();
    assert_eq!(skill.tree, tree);
    assert_eq!(skill.source, rig.library.personal().await.unwrap());
    let root = rig.dir.path().join("data/skills/personal");
    let log = sources::in_repo(&root, &["log", "--format=%s"]).unwrap();
    assert_eq!(log.trim(), "Take unslop from the global skills");
}

#[tokio::test]
async fn someone_elses_source_takes_no_commits() {
    let rig = Rig::on("macos");
    let remote = repo(&rig.dir.path().join("remote"), &[("alpha", "First")]);
    let theirs = rig
        .library
        .add(rig.git(&remote, SourceTrust::ThirdParty))
        .await
        .unwrap();
    let folder = rig.library.clone_dir(theirs.id).join("skills/alpha");
    let refused = rig
        .library
        .commit_folder(theirs.id, "alpha", &folder, "Change")
        .await
        .unwrap_err();
    assert!(refused.contains("your own sources"), "{refused}");
    let bad = rig
        .library
        .commit_folder(theirs.id, "Bad Name", &folder, "Change")
        .await
        .unwrap_err();
    assert!(bad.starts_with("The name Bad Name"), "{bad}");
}

#[tokio::test]
async fn a_commit_pushes_to_its_remote_when_asked() {
    let rig = Rig::on("macos");
    let made = repo(&rig.dir.path().join("made"), &[("alpha", "First")]);
    let remote = rig.dir.path().join("remote.git");
    let dir = rig.dir.path().to_path_buf();
    sources::in_repo(&dir, &["clone", "--quiet", "--bare", "made", "remote.git"]).unwrap();
    let mine = rig
        .library
        .add(rig.git(&remote, SourceTrust::Own))
        .await
        .unwrap();
    let folder = rig.dir.path().join("new/alpha");
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join(SKILL_FILE),
        "---\nname: alpha\ndescription: Second\n---\n",
    )
    .unwrap();
    rig.library
        .commit_folder(mine.id, "alpha", &folder, "Change alpha")
        .await
        .unwrap();
    let before = sources::in_repo(&remote, &["log", "--format=%s"]).unwrap();
    assert_eq!(before.trim(), "skills");
    let unpushed = |rig: &Rig| rig.library.find(mine.id).unwrap().unpushed;
    assert!(unpushed(&rig));
    assert!(rig.published().unwrap().sources[0].unpushed);

    rig.library.push(mine.id).await.unwrap();
    assert!(!unpushed(&rig));
    let after = sources::in_repo(&remote, &["log", "--format=%s %an"]).unwrap();
    assert_eq!(after.lines().next(), Some("Change alpha Test"));
    drop(made);

    // The personal source has nowhere to push.
    let personal = rig.library.personal().await.unwrap();
    let refused = rig.library.push(personal).await.unwrap_err();
    assert_eq!(refused, "The source has no remote to push to.");
}

#[tokio::test]
async fn a_device_takes_the_library_over_with_the_same_ids() {
    let old = Rig::on("macos");
    let remote = repo(&old.dir.path().join("remote"), &[("alpha", "First")]);
    let git = old
        .library
        .add(old.git(&remote, SourceTrust::Own))
        .await
        .unwrap();
    let personal = old.library.personal().await.unwrap();
    let folder = old.dir.path().join("new/mine");
    write_skills(&folder, &[("mine", "Mine")]);
    old.library
        .commit_folder(personal, "mine", &folder.join("skills/mine"), "Add mine")
        .await
        .unwrap();
    let local = old.dir.path().join("folder");
    write_skills(&local, &[("beta", "Second")]);
    let folder_source = old
        .library
        .add(NewSource {
            name: "Folder".into(),
            kind: SourceKind::Local {
                path: local.to_string_lossy().into_owned(),
            },
            default_ref: None,
            trust: SourceTrust::Own,
        })
        .await
        .unwrap();

    let new = Rig::on("linux");
    new.hold(Some(true));
    let other = repo(&new.dir.path().join("other"), &[("gamma", "Third")]);
    let gone = new
        .library
        .add(new.git(&other, SourceTrust::ThirdParty))
        .await
        .unwrap();
    new.hold(None);
    // The app copies each tree the new holder lacks first.
    let sources = old.library.sources();
    for skill in sources.iter().flat_map(|source| &source.skills) {
        let tree = old.library.store().path(&skill.tree).unwrap();
        new.library.store().copy_in(&tree).unwrap();
    }

    new.library.take("Mac", sources.clone()).await.unwrap();
    assert_eq!(new.published(), None, "it waits for the setting");
    let mut ids: Vec<_> = new.library.sources().iter().map(|s| s.id).collect();
    ids.sort();
    let mut wanted = vec![git.id, personal, folder_source.id];
    wanted.sort();
    assert_eq!(ids, wanted);
    assert!(!new.library.clone_dir(gone.id).exists());

    let taken = new.library.find(git.id).unwrap();
    assert!(new.library.clone_dir(git.id).join(".git").is_dir());
    assert_eq!(
        (taken.name.as_str(), taken.trust),
        ("Skills", SourceTrust::Own)
    );
    assert_eq!(taken.skills, old.library.find(git.id).unwrap().skills);
    assert_eq!(taken.problem, None);

    let mine = new.library.find(personal).unwrap();
    let root = new.dir.path().join("data/skills/personal");
    assert_eq!(
        mine.kind,
        SourceKind::Local {
            path: root.to_string_lossy().into_owned()
        }
    );
    let before = old.library.find(personal).unwrap().skills;
    assert_eq!(mine.skills, before);
    let log = sources::in_repo(&root, &["log", "--format=%s"]).unwrap();
    assert_eq!(log.trim(), "Take the personal skills from Mac");
    new.hold(Some(true));
    assert_eq!(new.library.personal().await.unwrap(), personal);

    let stays = new.library.find(folder_source.id).unwrap();
    assert_eq!(stays.problem.as_deref(), Some("This folder is on Mac."));
    assert_eq!(stays.skills, folder_source.skills);

    new.library.publish();
    let published = new.published().unwrap();
    assert_eq!(published.sources.len(), 3);
    assert_eq!(
        published.always_skills,
        old.published().unwrap().always_skills
    );
}

#[tokio::test]
async fn a_git_source_that_wont_clone_keeps_its_skills_and_says_why() {
    let old = Rig::on("macos");
    let remote = repo(&old.dir.path().join("remote"), &[("alpha", "First")]);
    let git = old
        .library
        .add(old.git(&remote, SourceTrust::ThirdParty))
        .await
        .unwrap();
    fs::remove_dir_all(&remote).unwrap();
    let new = Rig::on("linux");
    new.library.take("Mac", vec![git.clone()]).await.unwrap();
    let kept = new.library.find(git.id).unwrap();
    assert_eq!(kept.skills, git.skills);
    let problem = kept.problem.unwrap();
    assert!(
        problem.starts_with("Couldn't clone it on this device."),
        "{problem}"
    );
}

#[test]
fn a_commit_without_a_git_identity_says_so() {
    let said = unknown_author("git said: fatal: unable to auto-detect email address".into());
    assert!(
        said.starts_with("Git on this device doesn't know who you are"),
        "{said}"
    );
    assert_eq!(unknown_author("git said: no".into()), "git said: no");
}

#[tokio::test]
async fn the_sweep_keeps_what_a_source_or_a_lock_names() {
    let rig = Rig::on("macos");
    let remote = repo(&rig.dir.path().join("remote"), &[("alpha", "First")]);
    let added = rig
        .library
        .add(rig.git(&remote, SourceTrust::Own))
        .await
        .unwrap();
    let in_source = added.skills[0].tree.clone();
    let store = rig.library.store().clone();
    let skill = |name: &str| {
        let folder = rig.dir.path().join("loose").join(name);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(SKILL_FILE), format!("---\nname: {name}\n---\n")).unwrap();
        store.copy_in(&folder).unwrap().0
    };
    let locked = skill("locked");
    let unused = skill("unused");
    let record =
        serde_json::json!({ "lock": { "skills": [{ "name": "locked", "tree": locked.0 }] } });
    rig.db
        .lock()
        .execute(
            "INSERT INTO project_skills (id, record) VALUES ('p', ?1)",
            params![record.to_string()],
        )
        .unwrap();

    // Every tree was last named long before the clock's time.
    let long_ago = UNIX_EPOCH + Duration::from_millis(rig.clock.wall().0) - KEEP_FOR * 2;
    for tree in [&in_source, &locked, &unused] {
        let folder = fs::File::open(store.path(tree).unwrap()).unwrap();
        folder.set_modified(long_ago).unwrap();
    }
    rig.library.sweep().await;
    assert!(store.has(&in_source) && store.has(&locked));
    assert!(!store.has(&unused));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tidy_joins_a_tracked_repo_once() {
    use crate::skills::globals::Library as Tidy;

    let rig = Rig::on("macos");
    let remote = repo(
        &rig.dir.path().join("remote"),
        &[("tracked", "From a repo")],
    );
    let url = remote.to_string_lossy().into_owned();
    let library = rig.library.clone();
    tokio::task::spawn_blocking(move || {
        Tidy::join(&library, "owner/remote", &url)?;
        Tidy::join(&library, "owner/remote", &url)
    })
    .await
    .unwrap()
    .unwrap();

    let sources = rig.library.sources();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].name, "owner/remote");
    assert_eq!(sources[0].trust, SourceTrust::ThirdParty);
    assert_eq!(sources[0].skills[0].name, "tracked");
}

#[tokio::test]
async fn the_always_skills_it_holds_are_published() {
    let rig = Rig::on("macos");
    let remote = repo(&rig.dir.path().join("remote"), &[("unslop", "Plain words")]);
    rig.library
        .add(rig.git(&remote, SourceTrust::Own))
        .await
        .unwrap();
    rig.library.publish();
    let published = rig.published().unwrap();
    let names: Vec<&str> = published
        .always_skills
        .iter()
        .map(|skill| skill.name.as_str())
        .collect();
    assert_eq!(names, ["unslop"]);
    assert_eq!(published.always_skills[0].source, published.sources[0].id);
}

#[test]
fn a_device_keeps_the_always_skills_the_app_brought() {
    use crate::skills::globals::Library as Tidy;

    let rig = Rig::on("linux");
    let skill = |name: &str, tree: char| LockedSkill {
        name: name.into(),
        source: SkillSourceId::generate(),
        path: format!("skills/{name}"),
        commit: Some("c".repeat(40)),
        tree: TreeHash(tree.to_string().repeat(64)),
        pinned: false,
    };
    let unslop = skill("unslop", 'a');
    rig.library
        .set_always(vec![
            unslop.clone(),
            skill("unslop", 'b'),
            skill("propose-skill-change", 'c'),
            skill("other", 'd'),
        ])
        .unwrap();
    assert_eq!(
        Tidy::always_skill(&rig.library, "unslop"),
        Some(unslop.clone())
    );
    assert_eq!(
        Tidy::always_skill(&rig.library, "propose-skill-change"),
        None
    );
    assert!(rig.library.kept().unwrap().contains(&unslop.tree));

    // A restart reads them back.
    let reopened = Rig::open(&rig.dir, &rig.hub, &rig.db, &rig.clock);
    assert_eq!(Tidy::always_skill(&reopened, "unslop"), Some(unslop));
}
