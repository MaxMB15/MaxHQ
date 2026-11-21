// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The library this device keeps when it's the holder: its sources, read
//! into the catalog it publishes as the [`SkillLibrary`]. Every device runs
//! one for its store's sweep; only the holder adds, fetches and commits.
//!
//! The holder is a device setting, and a Mac holds it unless you say
//! otherwise, since that's where your git credentials and your skills are.
//! The clones live in `<data>/skills/sources/<source>/`.
//!
//! Any other device keeps the holder's always skills the app last brought
//! it, in `<data>/skills/always.json`, so its always set matches the
//! holder's.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, UNIX_EPOCH};

use maxhq_core::fs::{write_atomic, Atomic};
use maxhq_core::skills::store::{well_formed, TreeStore};
use maxhq_core::skills::SKILL_FILE;
use maxhq_core::skills::{always, frontmatter};
use maxhq_protocol::ids::SkillSourceId;
use maxhq_protocol::model::DeviceSettings;
use maxhq_protocol::skills::{
    LockedSkill, SkillInfo, SkillLibrary, SkillSource, SourceKind, SourceTrust, TreeHash,
};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use tokio::sync::broadcast::error::RecvError;
use tracing::warn;

use super::held::Held;
use super::sources;
use super::store::SkillStore;
use crate::db::Db;
use crate::hub::Hub;
use crate::session::clock::Clock;

/// What the personal source is called.
pub const PERSONAL: &str = "Personal";

/// The refusal for a library request on a device that doesn't keep it.
pub const NOT_HOLDER: &str = "This device doesn't keep the skill library.";

/// Where a device that doesn't hold the library keeps the holder's always
/// skills, under `<data>/skills`.
const RECEIVED: &str = "always.json";

/// How often the store is swept.
const SWEEP_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// What a source request asks for, before it has an ID.
#[derive(Clone, Debug)]
pub struct NewSource {
    /// What the app calls it.
    pub name: String,
    /// Where its skills come from.
    pub kind: SourceKind,
    /// The branch or tag to fetch.
    pub default_ref: Option<String>,
    /// Whose it is.
    pub trust: SourceTrust,
}

/// This device's library. Cloning it gives another handle to the same one.
#[derive(Clone)]
pub struct Library(Arc<Inner>);

struct Inner {
    hub: Hub,
    db: Db,
    store: Arc<SkillStore>,
    /// `<data>/skills`.
    root: PathBuf,
    clock: Arc<dyn Clock>,
    sources: Mutex<Vec<SkillSource>>,
    /// The holder's always skills, as the app last brought them.
    received: Mutex<Vec<LockedSkill>>,
    /// Held across each change, so two never run at once.
    busy: tokio::sync::Mutex<()>,
}

impl Library {
    /// The library under maxd's data folder, with the sources it stored.
    pub fn open(
        hub: Hub,
        db: Db,
        store: Arc<SkillStore>,
        data: &Path,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let sources = db.skill_sources().unwrap_or_else(|error| {
            warn!(%error, "couldn't read the skill sources");
            Vec::new()
        });
        let root = data.join("skills");
        let received = fs::read(root.join(RECEIVED))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self(Arc::new(Inner {
            hub,
            db,
            store,
            root,
            clock,
            sources: Mutex::new(sources),
            received: Mutex::new(received),
            busy: tokio::sync::Mutex::new(()),
        }))
    }

    /// The device's store.
    #[allow(dead_code)]
    pub fn store(&self) -> &Arc<SkillStore> {
        &self.0.store
    }

    /// Whether this device keeps the library.
    pub fn holds(&self) -> bool {
        let device = self.0.hub.device();
        match self.0.hub.get(&EntityKey::DeviceSettings(device.id)) {
            Some(Entity::DeviceSettings(DeviceSettings {
                skill_library: Some(holds),
                ..
            })) => holds,
            _ => device.os == "macos",
        }
    }

    /// The sources, as stored.
    pub fn sources(&self) -> Vec<SkillSource> {
        self.lock().clone()
    }

    /// Publishes the library while this device keeps it, and takes it
    /// away while it doesn't.
    pub fn publish(&self) {
        let device = self.0.hub.device().id;
        let change = if self.holds() {
            let mut sources = self.sources();
            sources.sort_by_key(|s| s.name.to_lowercase());
            Change::Upsert {
                value: Entity::SkillLibrary(SkillLibrary {
                    device,
                    sources,
                    always: always::NAMES.map(str::to_owned).to_vec(),
                    always_skills: always::NAMES
                        .iter()
                        .filter_map(|name| self.own_always(name))
                        .collect(),
                }),
            }
        } else {
            Change::Remove {
                key: EntityKey::SkillLibrary(device),
            }
        };
        self.0.hub.apply([change]);
    }

    /// Adds a source and reads its skills. Adding one that's there already
    /// gives it as it is.
    pub async fn add(&self, new: NewSource) -> Result<SkillSource, String> {
        let _busy = self.begin().await?;
        let name = new.name.trim().to_owned();
        if name.is_empty() {
            return Err("A source needs a name.".into());
        }
        let id = match &new.kind {
            SourceKind::Git { url } if url.trim().is_empty() => {
                return Err("A git source needs a URL.".into())
            }
            SourceKind::Git { url } => SkillSourceId::derive(&format!("git:{}", url.trim())),
            SourceKind::Local { path } if !Path::new(path).is_absolute() => {
                return Err("A local source needs an absolute path.".into())
            }
            SourceKind::Local { path } => SkillSourceId::derive(&format!("local:{path}")),
        };
        if let Some(found) = self.find(id) {
            return Ok(found);
        }
        let kind = match new.kind {
            SourceKind::Git { url } => SourceKind::Git {
                url: url.trim().to_owned(),
            },
            local => local,
        };
        let default_ref = new
            .default_ref
            .filter(|reference| !reference.trim().is_empty());
        let mut source = SkillSource {
            id,
            name,
            kind,
            default_ref,
            trust: new.trust,
            commit: None,
            fetched_at: None,
            problem: None,
            skills: Vec::new(),
            pack: None,
            unpushed: false,
        };
        if matches!(source.kind, SourceKind::Git { .. }) {
            self.clone_source(&source).await?;
        }
        if let Err(why) = self.reread(&mut source).await {
            self.forget_clone(&source);
            return Err(why);
        }
        self.keep(&source)?;
        Ok(source)
    }

    /// Fetches a source again and reads its skills. A failed fetch keeps
    /// the skills it had and says why on the source.
    pub async fn fetch(&self, id: SkillSourceId) -> Result<SkillSource, String> {
        let _busy = self.begin().await?;
        let mut source = self.find(id).ok_or("There's no such skill source.")?;
        let fetched = match &source.kind {
            SourceKind::Git { .. } if !self.clone_dir(id).join(".git").exists() => {
                self.clone_source(&source).await
            }
            SourceKind::Git { .. } => {
                let (dir, branch) = (self.clone_dir(id), source.default_ref.clone());
                self.blocking(move || sources::update(&dir, branch.as_deref()))
                    .await
            }
            SourceKind::Local { .. } => Ok(()),
        };
        let read = match fetched {
            Ok(()) => self.reread(&mut source).await,
            Err(why) => Err(why),
        };
        if let Err(why) = &read {
            source.problem = Some(why.clone());
        }
        self.keep(&source)?;
        read.map(|()| source)
    }

    /// Removes a source. Its clone goes; a local folder stays where it is,
    /// and the store keeps every tree a project locked.
    pub async fn remove(&self, id: SkillSourceId) -> Result<(), String> {
        let _busy = self.begin().await?;
        let Some(source) = self.find(id) else {
            return Ok(());
        };
        self.0
            .db
            .remove_skill_source(id)
            .map_err(|err| format!("couldn't forget the source: {err}"))?;
        self.lock().retain(|kept| kept.id != id);
        self.forget_clone(&source);
        self.publish();
        Ok(())
    }

    /// Puts the skill folder at `folder` into `source` as `name`, over the
    /// skill of that name if there is one, commits it when the source is a
    /// git repo, and gives the skill as the library now has it. `folder`
    /// stays as it was. Only your own sources take skills this way; a
    /// change to someone else's skill goes into the personal source under
    /// a name of its own.
    ///
    /// The commit stays local until [`Library::push`].
    pub async fn commit_folder(
        &self,
        source: SkillSourceId,
        name: &str,
        folder: &Path,
        message: &str,
    ) -> Result<SkillInfo, String> {
        let _busy = self.begin().await?;
        self.commit_locked(source, name, folder, message).await
    }

    /// Pushes a source's commits to its remote.
    pub async fn push(&self, id: SkillSourceId) -> Result<(), String> {
        let _busy = self.begin().await?;
        let mut source = self.find(id).ok_or("There's no such skill source.")?;
        let root = self.root_of(&source);
        let (pushed, unpushed) = self
            .blocking(move || {
                let pushed = sources::push(&root);
                Ok((pushed, sources::unpushed(&root)))
            })
            .await?;
        if source.unpushed != unpushed {
            source.unpushed = unpushed;
            self.keep(&source)?;
        }
        pushed
    }

    /// Takes the library over from `from`, the device that held it, with
    /// its sources under the same IDs, names, refs and trust. Each git
    /// source is cloned with this device's own git access; one that won't
    /// clone keeps the skills it had and says why. The personal source is
    /// rebuilt here from its skills' trees, which must be in this device's
    /// store, as one commit. Any other folder source stays as it was, with
    /// a problem, since its folder is on `from`. This device's own sources
    /// go. The library is published once the settings say this device
    /// holds it.
    pub async fn take(&self, from: &str, incoming: Vec<SkillSource>) -> Result<(), String> {
        let _busy = self.0.busy.lock().await;
        for gone in self.sources() {
            if !incoming.iter().any(|source| source.id == gone.id) {
                self.0
                    .db
                    .remove_skill_source(gone.id)
                    .map_err(|err| format!("couldn't forget the source: {err}"))?;
                self.lock().retain(|kept| kept.id != gone.id);
                self.forget_clone(&gone);
            }
        }
        let personal = incoming.iter().position(|source| {
            source.name == PERSONAL
                && source.trust == SourceTrust::Own
                && matches!(source.kind, SourceKind::Local { .. })
        });
        for (at, mut source) in incoming.into_iter().enumerate() {
            source.unpushed = false;
            let taken = match &source.kind {
                SourceKind::Git { .. } => match self.clone_source(&source).await {
                    Ok(()) => self.reread(&mut source).await,
                    Err(why) => Err(format!("Couldn't clone it on this device. {why}")),
                },
                SourceKind::Local { .. } if Some(at) == personal => {
                    self.take_personal(from, &mut source).await
                }
                SourceKind::Local { .. } => Err(format!("This folder is on {from}.")),
            };
            if let Err(why) = taken {
                source.problem = Some(why);
            }
            self.keep(&source)?;
        }
        Ok(())
    }

    /// The personal source, made as a local git repo under maxd's data
    /// folder the first time it's asked for.
    pub async fn personal(&self) -> Result<SkillSourceId, String> {
        let _busy = self.begin().await?;
        self.personal_locked().await
    }

    /// Keeps the holder's always skills the app brought, each in the store
    /// already. A name outside the always set, or one MaxHQ ships, is left
    /// out.
    pub fn set_always(&self, skills: Vec<LockedSkill>) -> Result<(), String> {
        let mut kept: Vec<LockedSkill> = Vec::new();
        for skill in skills {
            let wanted = always::NAMES.contains(&skill.name.as_str())
                && always::builtin(&skill.name).is_none()
                && well_formed(&skill.tree);
            if wanted && !kept.iter().any(|k| k.name == skill.name) {
                kept.push(skill);
            }
        }
        let json = serde_json::to_vec_pretty(&kept).map_err(|err| err.to_string())?;
        let mut received = self.received();
        fs::create_dir_all(&self.0.root).map_err(|err| err.to_string())?;
        write_atomic(&self.0.root.join(RECEIVED), &json, Atomic::exact(0o600))
            .map_err(|err| format!("couldn't keep the always skills: {err}"))?;
        *received = kept;
        Ok(())
    }

    /// Every tree a source, the always set or a project's lock names,
    /// which the sweep keeps, or none when the locks can't be read.
    pub fn kept(&self) -> Option<HashSet<TreeHash>> {
        let mut kept: HashSet<TreeHash> = self
            .lock()
            .iter()
            .flat_map(|source| {
                let skills = source.skills.iter().map(|skill| skill.tree.clone());
                skills.chain(source.pack.clone())
            })
            .chain(self.received().iter().map(|skill| skill.tree.clone()))
            .collect();
        match self.0.db.project_skill_records() {
            Ok(records) => records
                .iter()
                .for_each(|record| trees_in(record, &mut kept)),
            Err(error) => {
                warn!(%error, "couldn't read the project skills");
                return None;
            }
        }
        Some(kept)
    }

    /// Sweeps the store once. Without the locks nothing is known unused,
    /// so it waits for the next time.
    pub async fn sweep(&self) {
        let Some(kept) = self.kept() else {
            return;
        };
        let now = UNIX_EPOCH + Duration::from_millis(self.0.clock.wall().0);
        let store = self.0.store.clone();
        let swept = self.blocking(move || Ok(store.sweep(&kept, now))).await;
        if let Ok(swept) = swept
            && !swept.is_empty()
        {
            tracing::info!(trees = swept.len(), "swept unused skill trees");
        }
    }

    /// Publishes the library and sweeps the store, then follows the
    /// holder setting and sweeps each day, for as long as `maxd` runs.
    pub async fn watch(self) {
        let mut deltas = self.0.hub.subscribe(None).deltas;
        let mut held = self.holds();
        self.publish();
        self.sweep().await;
        let mut next = self.0.clock.now() + SWEEP_EVERY;
        loop {
            tokio::select! {
                () = self.0.clock.sleep_until(next) => {
                    self.sweep().await;
                    next = self.0.clock.now() + SWEEP_EVERY;
                }
                delta = deltas.recv() => match delta {
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return,
                },
            }
            let holds = self.holds();
            if holds != held {
                held = holds;
                self.publish();
            }
        }
    }

    async fn commit_locked(
        &self,
        id: SkillSourceId,
        name: &str,
        folder: &Path,
        message: &str,
    ) -> Result<SkillInfo, String> {
        frontmatter::check_name(name).map_err(|why| format!("The name {name} {why}."))?;
        if !folder.join(SKILL_FILE).is_file() {
            return Err(format!("The folder has no {SKILL_FILE}."));
        }
        let mut source = self.find(id).ok_or("There's no such skill source.")?;
        if source.trust != SourceTrust::Own {
            return Err("Skills can only be committed to your own sources.".into());
        }
        let root = self.root_of(&source);
        let path = match source.skills.iter().find(|skill| skill.name == name) {
            Some(skill) => skill.path.clone(),
            None => next_to(&source.skills, name),
        };
        let store = self.0.store.clone();
        let (from, message) = (folder.to_path_buf(), message.to_owned());
        let at = path.clone();
        self.blocking(move || {
            // The folder is read once, refusing links, and what goes into
            // the source is what was hashed, whatever the folder holds by now.
            let (_, held) = store.copy_in(&from)?;
            put(&root, &at, &held)?;
            commit(&root, &at, &message)
        })
        .await?;
        self.reread(&mut source).await?;
        self.keep(&source)?;
        source
            .skills
            .into_iter()
            .find(|skill| skill.path == path)
            .ok_or_else(|| format!("{name} was committed but couldn't be read back."))
    }

    /// The personal source's ID, which is another device's when this one
    /// took the library over.
    async fn personal_locked(&self) -> Result<SkillSourceId, String> {
        let dir = self.0.root.join("personal");
        let path = dir.to_string_lossy().into_owned();
        let here = self.lock().iter().find_map(|source| match &source.kind {
            SourceKind::Local { path: at } if *at == path => Some(source.id),
            _ => None,
        });
        if let Some(id) = here {
            return Ok(id);
        }
        let id = SkillSourceId::derive(&format!("local:{path}"));
        self.blocking(move || made_personal(&dir)).await?;
        let source = SkillSource {
            id,
            name: PERSONAL.into(),
            kind: SourceKind::Local { path },
            default_ref: None,
            trust: SourceTrust::Own,
            commit: None,
            fetched_at: None,
            problem: None,
            skills: Vec::new(),
            pack: None,
            unpushed: false,
        };
        self.keep(&source)?;
        Ok(id)
    }

    /// Rebuilds the personal source `from` held in this device's own
    /// personal folder, from its skills' trees in the store, and reads it.
    async fn take_personal(&self, from: &str, source: &mut SkillSource) -> Result<(), String> {
        let dir = self.0.root.join("personal");
        let (store, skills) = (self.0.store.clone(), source.skills.clone());
        let message = format!("Take the personal skills from {from}");
        let made = dir.clone();
        self.blocking(move || {
            made_personal(&made)?;
            for (path, folder) in sources::skill_folders(&made) {
                if !skills.iter().any(|skill| skill.path == path) {
                    fs::remove_dir_all(&folder)
                        .map_err(|err| format!("couldn't clear the personal source: {err}"))?;
                }
            }
            for skill in &skills {
                let tree = store
                    .path(&skill.tree)
                    .ok_or_else(|| format!("{} didn't reach this device.", skill.name))?;
                let (_, held) = store.copy_in(&tree)?;
                put(&made, &skill.path, &held)?;
            }
            commit(&made, ".", &message)
        })
        .await?;
        source.kind = SourceKind::Local {
            path: dir.to_string_lossy().into_owned(),
        };
        self.reread(source).await
    }

    /// Clones a git source afresh into its clone folder.
    async fn clone_source(&self, source: &SkillSource) -> Result<(), String> {
        let SourceKind::Git { url } = &source.kind else {
            return Ok(());
        };
        let dir = self.clone_dir(source.id);
        let (url, branch) = (url.clone(), source.default_ref.clone());
        self.blocking(move || {
            let _ = fs::remove_dir_all(&dir);
            let cloned = sources::clone(&url, branch.as_deref(), &dir);
            if cloned.is_err() {
                let _ = fs::remove_dir_all(&dir);
            }
            cloned
        })
        .await
    }

    /// Reads a source's skills where its files are now, without fetching.
    async fn reread(&self, source: &mut SkillSource) -> Result<(), String> {
        let root = self.root_of(source);
        let (id, store) = (source.id, self.0.store.clone());
        let is_git = matches!(source.kind, SourceKind::Git { .. }) || root.join(".git").exists();
        let scratch = self
            .0
            .root
            .join("incoming")
            .join(uuid::Uuid::now_v7().simple().to_string());
        let remote = matches!(source.kind, SourceKind::Git { .. });
        let (read, commit, pack, unpushed) = self
            .blocking(move || {
                if !root.is_dir() {
                    return Err(format!("{} isn't there.", root.display()));
                }
                let commit = if is_git {
                    sources::head(&root).ok()
                } else {
                    None
                };
                let pack = match commit {
                    Some(_) => sources::pack(&root, &scratch, &store),
                    None => Ok(None),
                };
                let unpushed = remote && sources::unpushed(&root);
                Ok((sources::read(&root, id, &store), commit, pack, unpushed))
            })
            .await?;
        source.unpushed = unpushed;
        source.pack = pack.unwrap_or_else(|error| {
            warn!(%error, "couldn't read the source's pack");
            None
        });
        source.problem = read.problem();
        source.skills = read.skills;
        source.commit = commit;
        source.fetched_at = Some(self.0.clock.wall());
        Ok(())
    }

    /// Stores a source and publishes the library.
    fn keep(&self, source: &SkillSource) -> Result<(), String> {
        self.0
            .db
            .put_skill_source(source)
            .map_err(|err| format!("couldn't store the source: {err}"))?;
        {
            let mut sources = self.lock();
            match sources.iter_mut().find(|kept| kept.id == source.id) {
                Some(kept) => *kept = source.clone(),
                None => sources.push(source.clone()),
            }
        }
        self.publish();
        Ok(())
    }

    fn find(&self, id: SkillSourceId) -> Option<SkillSource> {
        self.lock().iter().find(|source| source.id == id).cloned()
    }

    /// Where a source's files are.
    fn root_of(&self, source: &SkillSource) -> PathBuf {
        match &source.kind {
            SourceKind::Git { .. } => self.clone_dir(source.id),
            SourceKind::Local { path } => PathBuf::from(path),
        }
    }

    fn clone_dir(&self, id: SkillSourceId) -> PathBuf {
        self.0.root.join("sources").join(id.to_string())
    }

    fn forget_clone(&self, source: &SkillSource) {
        if matches!(source.kind, SourceKind::Git { .. }) {
            let dir = self.clone_dir(source.id);
            if let Err(error) = fs::remove_dir_all(&dir)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                warn!(%error, path = %dir.display(), "couldn't remove a source's clone");
            }
        }
    }

    /// Waits for the change before this one, once this device is the
    /// holder.
    /// Holds the library's lock, so a test can stop a commit before it starts.
    #[cfg(test)]
    pub(crate) async fn busy(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.0.busy.lock().await
    }

    async fn begin(&self) -> Result<tokio::sync::MutexGuard<'_, ()>, String> {
        if !self.holds() {
            return Err(NOT_HOLDER.into());
        }
        Ok(self.0.busy.lock().await)
    }

    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        tokio::task::spawn_blocking(work)
            .await
            .unwrap_or_else(|err| Err(format!("the work stopped: {err}")))
    }

    fn lock(&self) -> MutexGuard<'_, Vec<SkillSource>> {
        self.0
            .sources
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The first of this library's sources' skills named `name`, for the
    /// always set.
    fn own_always(&self, name: &str) -> Option<LockedSkill> {
        self.lock().iter().find_map(|source| {
            let skill = source.skills.iter().find(|skill| skill.name == name)?;
            Some(LockedSkill {
                name: skill.name.clone(),
                source: source.id,
                path: skill.path.clone(),
                commit: source.commit.clone(),
                tree: skill.tree.clone(),
                pinned: false,
            })
        })
    }

    fn received(&self) -> MutexGuard<'_, Vec<LockedSkill>> {
        self.0
            .received
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Where a new skill goes in a source: beside its first skill, or at its
/// root when it has none.
fn next_to(skills: &[SkillInfo], name: &str) -> String {
    match skills.first().and_then(|skill| skill.path.rsplit_once('/')) {
        Some((parent, _)) => format!("{parent}/{name}"),
        None => name.to_owned(),
    }
}

/// The personal source's folder at `dir`, made as a git repo if it isn't
/// one yet.
fn made_personal(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|err| format!("couldn't make the personal source: {err}"))?;
    if !dir.join(".git").exists() {
        sources::in_repo(dir, &["init", "--quiet"])?;
    }
    Ok(())
}

/// Puts the files `held` read at `path` in `root`, in place of what's
/// there. A store copy is read-only, and a source stays editable.
fn put(root: &Path, path: &str, held: &Held) -> Result<(), String> {
    let failed = |err: std::io::Error| format!("couldn't write the skill: {err}");
    let target = root.join(path);
    let parent = target.parent().ok_or("The skill has no folder to go in.")?;
    fs::create_dir_all(parent).map_err(failed)?;
    let id = uuid::Uuid::now_v7().simple().to_string();
    let new = parent.join(format!(".maxhq-new-{id}"));
    let old = parent.join(format!(".maxhq-old-{id}"));
    let written = (|| {
        held.write(&new)?;
        if target.exists() {
            fs::rename(&target, &old)?;
        }
        fs::rename(&new, &target)
    })();
    let _ = fs::remove_dir_all(&new);
    let _ = fs::remove_dir_all(&old);
    written.map_err(failed)
}

/// Commits `path` in the repo at `root`, when `root` is a repo and the
/// files changed.
fn commit(root: &Path, path: &str, message: &str) -> Result<(), String> {
    if !root.join(".git").exists() {
        return Ok(());
    }
    sources::in_repo(root, &["add", "-A", "--", path])?;
    let staged = sources::in_repo(root, &["diff", "--cached", "--name-only", "--", path])?;
    if staged.trim().is_empty() {
        return Ok(());
    }
    #[cfg(test)]
    let who = ["-c", "user.name=Test", "-c", "user.email=test@example.com"];
    #[cfg(not(test))]
    let who: [&str; 0] = [];
    let mut args: Vec<&str> = who.to_vec();
    args.extend(["commit", "--quiet", "-m", message, "--", path]);
    sources::in_repo(root, &args)
        .map(drop)
        .map_err(unknown_author)
}

/// A commit's failure in words, saying so when git doesn't know who's
/// committing. It never makes an author up.
fn unknown_author(why: String) -> String {
    let unknown = [
        "Please tell me who you are",
        "auto-detect email",
        "empty ident",
    ];
    if unknown.iter().any(|said| why.contains(said)) {
        return "Git on this device doesn't know who you are, so it can't commit as you. \
            Set user.name and user.email in git's config and try again."
            .into();
    }
    why
}

/// The tidy takes global skills into the personal source, and adds the
/// repos tracked ones came from. It runs off the loop, so it waits on the
/// library's calls in place.
impl super::globals::Library for Library {
    fn join(&self, name: &str, url: &str) -> Result<(), String> {
        let new = NewSource {
            name: name.to_owned(),
            kind: SourceKind::Git {
                url: url.to_owned(),
            },
            default_ref: None,
            trust: SourceTrust::ThirdParty,
        };
        tokio::runtime::Handle::current().block_on(async { self.add(new).await.map(drop) })
    }

    fn commit_folder(&self, name: &str, from: &Path) -> Result<TreeHash, String> {
        tokio::runtime::Handle::current().block_on(async {
            let personal = self.personal().await?;
            let message = format!("Take {name} from the global skills");
            let skill = self.commit_folder(personal, name, from, &message).await?;
            Ok(skill.tree)
        })
    }

    /// The library's own when it has one, else the holder's as the app
    /// brought it.
    fn always_skill(&self, name: &str) -> Option<LockedSkill> {
        self.own_always(name).or_else(|| {
            let received = self.received();
            received.iter().find(|skill| skill.name == name).cloned()
        })
    }
}

/// Every well-formed tree hash under a `"tree"` key in `value`.
fn trees_in(value: &serde_json::Value, kept: &mut HashSet<TreeHash>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, inner) in map {
                match inner {
                    serde_json::Value::String(text) if key == "tree" => {
                        let tree = TreeHash(text.clone());
                        if well_formed(&tree) {
                            kept.insert(tree);
                        }
                    }
                    _ => trees_in(inner, kept),
                }
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|item| trees_in(item, kept)),
        _ => {}
    }
}

#[cfg(test)]
mod tests;
