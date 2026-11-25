// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Tidying this device's global skills once you say yes, and putting them
//! back. The plan only reads. An apply adds the repos tracked skills came
//! from as sources, moves your other skills into the library and every
//! folder or link it takes into
//! `<maxd data>/skills/backup/<date>/`, then links the always set into the
//! store. Nothing is deleted, and an undo puts the last tidy back. Only
//! skill folders and links are touched: no CLI settings file is opened,
//! and `~/.cursor` is never read.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use maxhq_core::fs::{write_atomic, Atomic};
use maxhq_core::fs_ext::{self, symlink};
use maxhq_core::skills::always;
use maxhq_core::skills::disk::DiskFs;
use maxhq_core::skills::globals::{self, GlobalsInput, Tidy};
use maxhq_core::skills::store::TreeStore;
use maxhq_protocol::ids::SkillSourceId;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::skills::{GlobalsPlan, LockedSkill, TreeHash};
use maxhq_protocol::time::Timestamp;
use serde::{Deserialize, Serialize};

/// How the tidy puts a skill into the library. The library's `add` and
/// `commit_folder` stand behind it.
pub trait Library: Send + Sync {
    /// Adds the git repo at `url` as a third-party source called `name`,
    /// unless it's one already.
    fn join(&self, name: &str, url: &str) -> Result<(), String>;

    /// Hashes the skill folder at `from`, lands it in this device's store
    /// and adds it to the library as `name`, and says its tree. `from` is
    /// a copy the call may move or remove.
    fn commit_folder(&self, name: &str, from: &Path) -> Result<TreeHash, String>;

    /// The skill the library keeps for the always skill `name`, if any.
    fn always_skill(&self, name: &str) -> Option<LockedSkill>;
}

/// The store and library the tidy uses, once `maxd` has them.
static WIRED: OnceLock<(Arc<dyn TreeStore>, Arc<dyn Library>)> = OnceLock::new();

/// One tidy or undo at a time.
static BUSY: Mutex<()> = Mutex::new(());

/// Hands the tidy this device's store and library. Until then its requests
/// are refused.
pub fn wire(store: Arc<dyn TreeStore>, library: Arc<dyn Library>) {
    let _ = WIRED.set((store, library));
}

/// The always set for the delivery, each skill in the store, or none
/// until the store and library are wired.
pub fn always_locked() -> Vec<LockedSkill> {
    Globals::for_device().map_or_else(Vec::new, |globals| globals.always_locked())
}

/// This device's store, once wired.
pub fn store() -> Option<Arc<dyn TreeStore>> {
    WIRED.get().map(|(store, _)| store.clone())
}

/// The folder a store keeps its trees in.
pub fn store_root(store: &dyn TreeStore) -> Option<PathBuf> {
    let any = TreeHash("0".repeat(64));
    store.path(&any)?.parent().map(Path::to_path_buf)
}

/// What a tidy did, kept beside its backup so an undo can put it back.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Record {
    /// The plan it applied.
    plan_hash: String,
    /// Each folder or link it took, and where it went.
    moved: Vec<Moved>,
    /// Each link it made, and where it points.
    linked: Vec<Moved>,
}

/// One path and another.
#[derive(Debug, Serialize, Deserialize)]
struct Moved {
    from: PathBuf,
    to: PathBuf,
}

/// The record of a tidy, in its backup folder.
const RECORD: &str = "tidy.json";

/// The record once its tidy is undone.
const UNDONE: &str = "undone.json";

/// What the source of MaxHQ's own always skills is derived from.
const SHIPPED: &str = "maxhq";

/// The global skill folders on this device, and where the tidy keeps what
/// it takes.
pub struct Globals {
    /// The home folder.
    pub home: PathBuf,
    /// Each Claude config folder, the default first.
    pub claude_configs: Vec<PathBuf>,
    /// Each `CODEX_HOME`.
    pub codex_homes: Vec<PathBuf>,
    /// `<maxd data>/skills`, where the backups and the staging folder go.
    pub data: PathBuf,
    /// The device's store.
    pub store: Arc<dyn TreeStore>,
    /// The library.
    pub library: Arc<dyn Library>,
}

impl Globals {
    /// This device's folders, once the store and library are wired.
    pub fn for_device() -> Option<Self> {
        let (store, library) = WIRED.get()?;
        let home = std::env::home_dir()?;
        Some(Self {
            claude_configs: crate::observe::claude::config::config_dirs()
                .into_iter()
                .map(|dir| dir.path)
                .collect(),
            codex_homes: crate::usage::codex::homes()
                .into_iter()
                .map(|home| home.dir)
                .collect(),
            data: maxhq_core::paths::data_dir()?.join("skills"),
            store: store.clone(),
            library: library.clone(),
            home,
        })
    }

    /// The always set with the tree each has on this device: MaxHQ's own,
    /// or the library's when the store has it.
    pub fn always(&self) -> Vec<(String, Option<TreeHash>)> {
        always::NAMES
            .iter()
            .map(|name| {
                let tree = self.always_skill(name).map(|skill| skill.tree);
                ((*name).to_owned(), tree)
            })
            .collect()
    }

    /// The always skill `name` as this device has it: MaxHQ's own, or the
    /// library's when the store has it. A device that doesn't hold the
    /// library has the holder's, once the app brought it.
    fn always_skill(&self, name: &str) -> Option<LockedSkill> {
        match always::builtin_tree(name) {
            Some(tree) => Some(LockedSkill {
                name: name.to_owned(),
                source: SkillSourceId::derive(SHIPPED),
                path: name.to_owned(),
                commit: None,
                tree,
                pinned: false,
            }),
            None => self
                .library
                .always_skill(name)
                .filter(|skill| self.store.has(&skill.tree)),
        }
    }

    /// The always set for the delivery, each skill in the store. Landing
    /// MaxHQ's own is the only write, and it's to `maxd`'s own folder.
    pub fn always_locked(&self) -> Vec<LockedSkill> {
        always::NAMES
            .iter()
            .filter_map(|name| self.always_skill(name))
            .filter(|skill| match self.land_builtin(&skill.name, &skill.tree) {
                Ok(()) => true,
                Err(err) => {
                    tracing::warn!(%err, skill = %skill.name, "couldn't land an always skill");
                    false
                }
            })
            .collect()
    }

    fn input(&self) -> Result<GlobalsInput, String> {
        Ok(GlobalsInput {
            home: self.home.clone(),
            claude_configs: self.claude_configs.clone(),
            codex_homes: self.codex_homes.clone(),
            store: store_root(self.store.as_ref()).ok_or("the skill store has no folder")?,
            always: self.always(),
            tracked: super::seed::tracked(&self.home),
        })
    }

    /// What tidying would do. It writes nothing.
    pub fn plan(&self) -> Result<Tidy, String> {
        Ok(globals::plan(&DiskFs, &self.input()?))
    }

    fn backups(&self) -> PathBuf {
        self.data.join("backup")
    }

    /// The newest tidy that isn't undone, with its folder.
    fn last(&self) -> Option<(PathBuf, Record)> {
        let mut dirs: Vec<PathBuf> = fs::read_dir(self.backups())
            .ok()?
            .filter_map(|entry| Some(entry.ok()?.path()))
            .collect();
        dirs.sort();
        let dir = dirs
            .into_iter()
            .rev()
            .find(|dir| dir.join(RECORD).is_file())?;
        let record = serde_json::from_slice(&fs::read(dir.join(RECORD)).ok()?).ok()?;
        Some((dir, record))
    }

    /// What tidying would do, as the app shows it.
    pub fn shown(&self) -> Result<GlobalsPlan, String> {
        let mut plan = self.plan()?.plan();
        plan.undo = self.last().is_some();
        Ok(plan)
    }

    /// Does what the plan named `plan_hash` said, with your `picks` among
    /// copies that differ, as long as the folders are still as they were.
    /// `now` dates the backup.
    pub fn apply(&self, plan_hash: &str, picks: &[String], now: Timestamp) -> Result<(), String> {
        let _busy = BUSY.lock().unwrap_or_else(|poison| poison.into_inner());
        let mut tidy = self.plan()?;
        if tidy.hash() != plan_hash {
            // A resend of the apply that just ran.
            if self
                .last()
                .is_some_and(|(_, record)| record.plan_hash == plan_hash)
            {
                return Ok(());
            }
            return Err(
                "Your global skills changed since that plan. Look at the new one \
                        and apply it again."
                    .into(),
            );
        }
        tidy.pick(picks)?;
        if tidy.is_empty() {
            return Ok(());
        }
        for (name, tree) in self.always() {
            if let Some(tree) = tree {
                self.land_builtin(&name, &tree)?;
            }
        }
        // Everything goes into the library before anything moves, so a
        // failure here leaves the folders as they were.
        for join in &tidy.joins {
            self.library
                .join(&globals::source_name(&join.url), &join.url)
                .map_err(|why| format!("couldn't add {}: {why}", join.url))?;
        }
        let staging = self.data.join("staging");
        for taken in &tidy.moves {
            let copy = staging.join(uuid::Uuid::now_v7().simple().to_string());
            let real = fs::canonicalize(&taken.path).map_err(|err| failed(&taken.path, err))?;
            let committed = copy_folder(&real, &copy)
                .map_err(|err| failed(&taken.path, err))
                .and_then(|()| self.library.commit_folder(&taken.name, &copy));
            let _ = fs::remove_dir_all(&copy);
            if committed? != *taken.tree.as_ref().ok_or("a move has no tree")? {
                return Err(format!(
                    "{} changed while it was going into the library.",
                    taken.path.display()
                ));
            }
        }

        let dir = self.backup_dir(now)?;
        let mut record = Record {
            plan_hash: plan_hash.to_owned(),
            ..Record::default()
        };
        let done = self.take_and_link(&tidy, &dir, &mut record);
        let json = serde_json::to_vec_pretty(&record).map_err(|err| err.to_string())?;
        write_atomic(&dir.join(RECORD), &json, Atomic::exact(0o600))
            .map_err(|err| format!("couldn't keep the tidy's record: {err}"))?;
        done
    }

    /// Moves what the tidy takes into `dir`, then makes its links, noting
    /// each step in `record` as it goes.
    fn take_and_link(&self, tidy: &Tidy, dir: &Path, record: &mut Record) -> Result<(), String> {
        for taken in tidy.taken() {
            let to = dir.join(self.relative(&taken.path));
            make_parent(&to)?;
            fs::rename(&taken.path, &to).map_err(|err| failed(&taken.path, err))?;
            record.moved.push(Moved {
                from: taken.path.clone(),
                to,
            });
        }
        for link in &tidy.links {
            make_parent(&link.path)?;
            symlink(&link.target, &link.path).map_err(|err| failed(&link.path, err))?;
            record.linked.push(Moved {
                from: link.path.clone(),
                to: link.target.clone(),
            });
        }
        Ok(())
    }

    /// Puts back what the last tidy took, and takes away its links. The
    /// library keeps what went into it.
    pub fn undo(&self) -> Result<(), String> {
        let _busy = BUSY.lock().unwrap_or_else(|poison| poison.into_inner());
        let (dir, record) = self.last().ok_or("There's no tidy to undo.")?;
        for link in record.linked.iter().rev() {
            if fs_ext::read_link(&link.from).is_ok_and(|to| to == link.to) {
                fs_ext::remove_link(&link.from).map_err(|err| failed(&link.from, err))?;
            }
        }
        let mut left = Vec::new();
        for moved in record.moved.iter().rev() {
            if !there(&moved.to) {
                continue;
            }
            if there(&moved.from) {
                left.push(moved.from.display().to_string());
                continue;
            }
            make_parent(&moved.from)?;
            fs::rename(&moved.to, &moved.from).map_err(|err| failed(&moved.from, err))?;
        }
        if !left.is_empty() {
            return Err(format!(
                "Something new is in the way, so these stay in the backup: {}.",
                left.join(", ")
            ));
        }
        fs::rename(dir.join(RECORD), dir.join(UNDONE))
            .map_err(|err| format!("couldn't mark the tidy undone: {err}"))
    }

    /// Lands MaxHQ's own always skill `name` in the store, when it is one
    /// and the store lacks it.
    fn land_builtin(&self, name: &str, tree: &TreeHash) -> Result<(), String> {
        if always::builtin(name).is_none() || self.store.has(tree) {
            return Ok(());
        }
        let staging = self.data.join("staging");
        fs::create_dir_all(&staging).map_err(|err| failed(&staging, err))?;
        let dir = staging.join(uuid::Uuid::now_v7().simple().to_string());
        always::write_builtin(name, &dir).map_err(|err| failed(&dir, err))?;
        let landed = self.store.land(tree, &dir).map(drop);
        let _ = fs::remove_dir_all(&dir);
        landed
    }

    /// A new backup folder named for `now`.
    fn backup_dir(&self, now: Timestamp) -> Result<PathBuf, String> {
        let date = i64::try_from(now.0)
            .ok()
            .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
            .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
            .strftime("%Y-%m-%d-%H%M%S")
            .to_string();
        let backups = self.backups();
        fs::create_dir_all(&backups).map_err(|err| failed(&backups, err))?;
        for n in 1.. {
            let name = if n == 1 {
                date.clone()
            } else {
                format!("{date}-{n}")
            };
            let dir = backups.join(name);
            match fs::create_dir(&dir) {
                Ok(()) => return Ok(dir),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(failed(&dir, err)),
            }
        }
        unreachable!("the counter runs out first")
    }

    /// Where `path` goes inside a backup: its path from home, or from `/`.
    fn relative(&self, path: &Path) -> PathBuf {
        match path.strip_prefix(&self.home) {
            Ok(rest) => rest.to_path_buf(),
            Err(_) => Path::new("root").join(path.strip_prefix("/").unwrap_or(path)),
        }
    }
}

/// Whether anything, a dangling link too, is at `path`.
fn there(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn make_parent(path: &Path) -> Result<(), String> {
    match path.parent() {
        Some(parent) => fs::create_dir_all(parent).map_err(|err| failed(parent, err)),
        None => Ok(()),
    }
}

fn failed(path: &Path, err: io::Error) -> String {
    format!("couldn't change {}: {err}", path.display())
}

/// Copies the skill folder `from` to `to`, which mustn't exist. A skill
/// holds no links, so any it meets are left out.
fn copy_folder(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let at = to.join(entry.file_name());
        if kind.is_dir() {
            copy_folder(&entry.path(), &at)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), at)?;
        }
    }
    Ok(())
}

/// The answer to a request about the tidy, run off the loop.
fn answer(run: impl FnOnce(&Globals) -> ActionOutcome) -> ActionOutcome {
    match Globals::for_device() {
        Some(globals) => run(&globals),
        None => super::refused(),
    }
}

fn done(result: Result<(), String>) -> ActionOutcome {
    match result {
        Ok(()) => ActionOutcome::Done,
        Err(reason) => ActionOutcome::Refused { reason },
    }
}

/// Answers [`ClientMessage::SkillsGlobalsPlan`].
///
/// [`ClientMessage::SkillsGlobalsPlan`]: maxhq_protocol::message::ClientMessage::SkillsGlobalsPlan
pub fn plan_request() -> ActionOutcome {
    answer(|globals| match globals.shown() {
        Ok(plan) => ActionOutcome::SkillsGlobalsPlan { plan },
        Err(reason) => ActionOutcome::Refused { reason },
    })
}

/// Answers [`ClientMessage::SkillsGlobalsApply`].
///
/// [`ClientMessage::SkillsGlobalsApply`]: maxhq_protocol::message::ClientMessage::SkillsGlobalsApply
pub fn apply_request(plan_hash: String, picks: Vec<String>) -> ActionOutcome {
    answer(|globals| done(globals.apply(&plan_hash, &picks, Timestamp::now())))
}

/// Answers [`ClientMessage::SkillsGlobalsUndo`].
///
/// [`ClientMessage::SkillsGlobalsUndo`]: maxhq_protocol::message::ClientMessage::SkillsGlobalsUndo
pub fn undo_request() -> ActionOutcome {
    answer(|globals| done(globals.undo()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use maxhq_core::skills::path_text;
    use maxhq_core::skills::scan::{scan, ScanInput};
    use maxhq_core::skills::store::FolderStore;
    use maxhq_os::fsmode;
    use maxhq_protocol::model::Vendor;
    use maxhq_protocol::skills::{SkillMark, SkillPlace};

    use super::*;
    use crate::testing::TempDir;

    /// Commits into the store and remembers what came in.
    struct FakeLibrary {
        store: Arc<FolderStore>,
        commits: Mutex<Vec<(String, TreeHash)>>,
        joins: Mutex<Vec<(String, String)>>,
    }

    impl Library for FakeLibrary {
        fn join(&self, name: &str, url: &str) -> Result<(), String> {
            self.joins
                .lock()
                .unwrap()
                .push((name.to_owned(), url.to_owned()));
            Ok(())
        }

        fn commit_folder(&self, name: &str, from: &Path) -> Result<TreeHash, String> {
            let tree = maxhq_core::skills::tree::of(&DiskFs, from).map_err(|e| e.to_string())?;
            self.store.land(&tree, from)?;
            self.commits
                .lock()
                .unwrap()
                .push((name.to_owned(), tree.clone()));
            Ok(tree)
        }

        fn always_skill(&self, _name: &str) -> Option<LockedSkill> {
            None
        }
    }

    const NOW: Timestamp = Timestamp(1_793_000_000_000);

    fn skill(dir: &Path, body: &str) {
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!(
                "---\nname: {}\ndescription: {body}\n---\n",
                dir.file_name().unwrap().to_string_lossy()
            ),
        )
        .unwrap();
        let run = dir.join("scripts/run.sh");
        fs::write(&run, "#!/bin/sh\n").unwrap();
        fsmode::set_mode(&run, 0o755).unwrap();
    }

    /// A temp home laid out like the Mac's, with settings files beside the
    /// skills.
    fn setup(dir: &TempDir) -> (Globals, Arc<FakeLibrary>) {
        let home = dir.path().join("home");
        skill(&home.join(".claude/skills/git-flow"), "Branches.");
        skill(&home.join(".claude/skills/unslop"), "Cut AI tells.");
        skill(&home.join(".claude/skills/synced/notes"), "Synced.");
        skill(&home.join(".claude-work/skills/grill-me"), "Questions.");
        skill(&home.join(".agents/skills/git-flow"), "Branches.");
        skill(&home.join(".codex/skills/.system/imagegen"), "Images.");
        skill(&home.join(".codex/skills/unslop"), "An older cut.");
        fs::write(home.join(".claude/settings.json"), "{\"a\":1}").unwrap();
        fs::write(home.join(".claude/settings.local.json"), "{}").unwrap();
        fs::write(home.join(".codex/config.toml"), "model = \"x\"\n").unwrap();
        let data = dir.path().join("data/skills");
        let store = Arc::new(FolderStore::new(data.join("store")));
        let library = Arc::new(FakeLibrary {
            store: store.clone(),
            commits: Mutex::new(Vec::new()),
            joins: Mutex::new(Vec::new()),
        });
        let globals = Globals {
            claude_configs: vec![home.join(".claude"), home.join(".claude-work")],
            codex_homes: vec![home.join(".codex")],
            data,
            store,
            library: library.clone(),
            home,
        };
        (globals, library)
    }

    /// Every path under `root`, with its bytes, mode or link target.
    fn snapshot(root: &Path) -> BTreeMap<PathBuf, String> {
        let mut all = BTreeMap::new();
        let mut todo = vec![root.to_path_buf()];
        while let Some(dir) = todo.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                let meta = fs::symlink_metadata(&path).unwrap();
                let what = if meta.file_type().is_symlink() {
                    format!("link {}", fs::read_link(&path).unwrap().display())
                } else if meta.is_dir() {
                    todo.push(path.clone());
                    format!("dir {:o}", fsmode::mode_of(&meta))
                } else {
                    format!(
                        "file {:o} {:?} {:?}",
                        fsmode::mode_of(&meta),
                        meta.modified().unwrap(),
                        fs::read(&path).unwrap()
                    )
                };
                all.insert(path.strip_prefix(root).unwrap().to_path_buf(), what);
            }
        }
        all
    }

    #[test]
    fn a_plan_writes_nothing_and_an_apply_needs_its_hash() {
        let dir = TempDir::new();
        let (globals, library) = setup(&dir);
        let before = snapshot(dir.path());
        let tidy = globals.plan().unwrap();
        assert_eq!(snapshot(dir.path()), before);
        let home = &globals.home;
        let shown = tidy.plan();
        assert_eq!(
            shown.moves,
            [
                home.join(".claude/skills/unslop"),
                home.join(".claude/skills/git-flow"),
                home.join(".claude-work/skills/grill-me"),
                home.join(".agents/skills/git-flow"),
            ]
            .map(|p| path_text(&p))
        );
        assert_eq!(
            shown.backups,
            [path_text(&home.join(".codex/skills/unslop"))]
        );
        assert_eq!(shown.links.len(), 6, "{:?}", shown.links);

        let wrong = globals.apply("not-the-plan", &[], NOW).unwrap_err();
        assert!(wrong.contains("changed since that plan"), "{wrong}");
        assert_eq!(snapshot(dir.path()), before);
        assert!(library.commits.lock().unwrap().is_empty());
    }

    #[test]
    fn an_apply_moves_backs_up_and_links_and_leaves_settings_alone() {
        let dir = TempDir::new();
        let (globals, library) = setup(&dir);
        let home = globals.home.clone();
        let settings = |snap: &BTreeMap<PathBuf, String>| -> Vec<String> {
            [
                ".claude/settings.json",
                ".claude/settings.local.json",
                ".codex/config.toml",
            ]
            .iter()
            .map(|p| snap[&Path::new("home").join(p)].clone())
            .collect()
        };
        let before = snapshot(dir.path());
        let hash = globals.plan().unwrap().hash();
        globals.apply(&hash, &[], NOW).unwrap();

        let after = snapshot(dir.path());
        assert_eq!(settings(&after), settings(&before));
        let commits: Vec<String> = library
            .commits
            .lock()
            .unwrap()
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        assert_eq!(commits, ["unslop", "git-flow", "grill-me", "git-flow"]);
        let backup = globals.data.join("backup/2026-10-26-073320");
        for moved in [
            ".claude/skills/git-flow",
            ".codex/skills/unslop",
            ".agents/skills/git-flow",
        ] {
            assert!(backup.join(moved).join("SKILL.md").is_file(), "{moved}");
            assert!(!home.join(moved).exists(), "{moved}");
        }
        assert!(home.join(".claude/skills/synced/notes/SKILL.md").is_file());
        assert!(home
            .join(".codex/skills/.system/imagegen/SKILL.md")
            .is_file());
        for config in [".claude", ".claude-work", ".agents"] {
            for name in always::NAMES {
                let link = home.join(config).join("skills").join(name);
                let to = fs_ext::read_link(&link).unwrap();
                assert!(
                    to.starts_with(globals.data.join("store")),
                    "{}",
                    link.display()
                );
                assert!(link.join("SKILL.md").is_file());
            }
        }

        // One unslop for every CLI now, from the always set.
        let mut input = ScanInput::new(dir.path(), &home);
        input.codex_admin = None;
        input.always = always::NAMES.iter().map(|n| (*n).to_owned()).collect();
        input.store = Some(globals.data.join("store"));
        let seen = scan(&DiskFs, &input).skills;
        for cli in [Vendor::Claude, Vendor::Codex, Vendor::Cursor] {
            let unslop: Vec<_> = seen
                .iter()
                .filter(|s| s.cli == cli && s.name == "unslop")
                .collect();
            assert_eq!(unslop.len(), 1, "{cli:?} {unslop:?}");
            assert_eq!(unslop[0].place, SkillPlace::Always);
            assert_ne!(unslop[0].mark, Some(SkillMark::Conflict));
        }

        // A resend is done, and a fresh plan has nothing left.
        globals.apply(&hash, &[], NOW).unwrap();
        assert!(globals.plan().unwrap().is_empty());
    }

    #[test]
    fn an_undo_puts_everything_back_byte_for_byte() {
        let dir = TempDir::new();
        let (globals, _) = setup(&dir);
        let home = dir.path().join("home");
        let before = snapshot(&home);
        let hash = globals.plan().unwrap().hash();
        globals.apply(&hash, &[], NOW).unwrap();
        assert_ne!(snapshot(&home), before);

        globals.undo().unwrap();
        assert_eq!(snapshot(&home), before);
        assert_eq!(globals.undo().unwrap_err(), "There's no tidy to undo.");

        // A second tidy gets its own backup.
        let hash = globals.plan().unwrap().hash();
        globals.apply(&hash, &[], NOW).unwrap();
        assert!(globals.data.join("backup/2026-10-26-073320-2").is_dir());
    }

    #[test]
    fn an_undo_leaves_what_is_in_the_way_in_the_backup() {
        let dir = TempDir::new();
        let (globals, _) = setup(&dir);
        let home = globals.home.clone();
        let hash = globals.plan().unwrap().hash();
        globals.apply(&hash, &[], NOW).unwrap();
        skill(&home.join(".claude/skills/git-flow"), "A new one.");

        let left = globals.undo().unwrap_err();
        assert!(left.contains(".claude/skills/git-flow"), "{left}");
        let backup = globals.data.join("backup/2026-10-26-073320");
        assert!(backup.join(".claude/skills/git-flow").is_dir());
        assert!(home.join(".codex/skills/unslop").is_dir());
        assert!(
            fs::read_to_string(home.join(".claude/skills/git-flow/SKILL.md"))
                .unwrap()
                .contains("A new one.")
        );
    }

    #[test]
    fn a_tracked_skill_joins_its_repo_and_copies_that_differ_wait() {
        let dir = TempDir::new();
        let (globals, library) = setup(&dir);
        let home = globals.home.clone();
        skill(&home.join(".agents/skills/pdf"), "PDFs.");
        skill(&home.join(".agents/skills/grill-me"), "Other questions.");
        let url = "https://github.com/owner/skills.git";
        fs::write(
            home.join(super::super::seed::SKILL_LOCK),
            format!(r#"{{"skills":{{"pdf":{{"sourceUrl":"{url}"}}}}}}"#),
        )
        .unwrap();
        let shown = globals.shown().unwrap();
        assert_eq!(shown.joins.len(), 1);
        assert_eq!(shown.joins[0].name, "owner/skills");
        assert_eq!(shown.conflicts.len(), 1);
        assert_eq!(shown.conflicts[0].name, "grill-me");
        assert!(!shown.undo);

        globals.apply(&shown.hash, &[], NOW).unwrap();
        assert_eq!(
            *library.joins.lock().unwrap(),
            [("owner/skills".to_owned(), url.to_owned())]
        );
        let commits: Vec<String> = library
            .commits
            .lock()
            .unwrap()
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        assert!(!commits
            .iter()
            .any(|name| name == "pdf" || name == "grill-me"));
        assert!(!home.join(".agents/skills/pdf").exists());
        let backup = globals.data.join("backup/2026-10-26-073320");
        assert!(backup.join(".agents/skills/pdf/SKILL.md").is_file());
        // Both copies of grill-me stay until you pick one.
        assert!(home.join(".claude-work/skills/grill-me").is_dir());
        assert!(home.join(".agents/skills/grill-me").is_dir());

        let again = globals.shown().unwrap();
        assert!(again.undo);
        let pick = path_text(&home.join(".agents/skills/grill-me"));
        globals.apply(&again.hash, &[pick], NOW).unwrap();
        let last = library.commits.lock().unwrap().last().unwrap().0.clone();
        assert_eq!(last, "grill-me");
        assert!(!home.join(".claude-work/skills/grill-me").exists());
        assert!(!home.join(".agents/skills/grill-me").exists());
        assert!(globals.shown().unwrap().conflicts.is_empty());

        // Two undos put back both tidies, and then there's none to undo.
        globals.undo().unwrap();
        globals.undo().unwrap();
        assert!(home.join(".agents/skills/pdf/SKILL.md").is_file());
        assert!(home.join(".claude-work/skills/grill-me").is_dir());
        assert!(!globals.shown().unwrap().undo);
    }

    #[test]
    fn the_always_set_lands_the_shipped_skill() {
        let dir = TempDir::new();
        let (globals, _) = setup(&dir);
        let skills = globals.always_locked();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, always::PROPOSE);
        assert!(globals.store.has(&skills[0].tree));
    }

    #[test]
    fn requests_are_refused_until_the_library_is_wired() {
        assert_eq!(plan_request(), super::super::refused());
    }
}
