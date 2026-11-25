// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Putting a project's locked skills into one checkout on this device, as
//! read-only copies in `.maxhq/skills` with relative links beside them,
//! and keeping them out of git through `.git/info/exclude`. A project that
//! commits its copies keeps only the proposals out.
//!
//! A copy that no longer matches anything MaxHQ wrote was changed in the
//! checkout. It isn't overwritten: it moves to `.maxhq/proposals` and the
//! locked tree takes its place.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use maxhq_core::fs::{read_regular, read_text, write_atomic, Atomic};
use maxhq_core::fs_ext;
use maxhq_core::skills::disk::DiskFs;
use maxhq_core::skills::plan::{self, Clash, Layout, Step, EXCLUDE_HEADING};
use maxhq_core::skills::store::TreeStore;
use maxhq_core::skills::{lock, tree, LINK_DIRS, LOCK_MIRROR, PROPOSALS_DIR, SKILLS_DIR};
use maxhq_os::fsmode;
use maxhq_protocol::skills::{SkillLock, TreeHash};
use maxhq_transfer::beneath::{set_mode, Dir, FileType};

/// Numbers the temporary folders, so two writes never share one.
static TEMPS: AtomicU64 = AtomicU64::new(0);

/// One file or folder in a copy, as `stat` gives it: its path in the
/// copy, size, change time and mode.
type Stamp = (PathBuf, u64, Option<SystemTime>, u32);

/// What MaxHQ knows of the copies in one checkout, so a check that finds
/// nothing changed hashes nothing.
#[derive(Debug, Default)]
pub struct Seen {
    /// Each copy's stamps and the tree they had.
    copies: HashMap<String, (Vec<Stamp>, TreeHash)>,
    /// How many copies were hashed.
    pub hashed: usize,
}

/// A copy that changed in the checkout, kept as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drifted {
    /// The skill.
    pub name: String,
    /// The tree it was locked at, none for a skill the lock lacks.
    pub base: Option<TreeHash>,
    /// The changed folder's tree, none when it can't be hashed.
    pub tree: Option<TreeHash>,
    /// Where it was kept.
    pub kept: PathBuf,
}

/// What putting the skills in did.
#[derive(Debug, Default)]
pub struct Applied {
    /// Whether anything in the checkout changed.
    pub changed: bool,
    /// The copies that had changed.
    pub drift: Vec<Drifted>,
    /// What was in the way and left alone.
    pub clashes: Vec<Clash>,
}

impl Applied {
    /// The links that stopped skills landing in the checkout at `root`, as
    /// paths in it.
    pub fn outside(&self, root: &Path) -> Vec<String> {
        self.clashes
            .iter()
            .filter_map(|clash| match clash {
                Clash::Outside(link) => Some(
                    link.strip_prefix(root)
                        .unwrap_or(link)
                        .display()
                        .to_string(),
                ),
                _ => None,
            })
            .collect()
    }
}

/// Makes the checkout at `root` hold `lock` as copies from `store`.
pub fn checkout(
    store: &dyn TreeStore,
    root: &Path,
    lock: &SkillLock,
    seen: &mut Seen,
) -> Result<Applied, String> {
    if !root.is_dir() {
        return Err(format!("{} isn't a folder.", root.display()));
    }
    let failed = |what: &str, err: io::Error| format!("couldn't {what}: {err}");
    let mut applied = Applied::default();
    // Checking the copies already moves folders, so this comes first.
    if let Some(link) =
        plan::store_link(&DiskFs, root).map_err(|err| failed("read the checkout", err))?
    {
        applied.clashes.push(Clash::Outside(link));
        return Ok(applied);
    }
    let copies = check_copies(store, root, lock, seen, &mut applied)
        .map_err(|err| failed("check the skill copies", err))?;
    let plan = plan::checkout(&DiskFs, root, lock, Layout::Copies, &copies)
        .map_err(|err| failed("read the checkout", err))?;
    for step in &plan.steps {
        if let Step::Copy { tree, .. } = step
            && !store.has(tree)
        {
            return Err(format!(
                "This device doesn't have the skill tree {} yet.",
                tree.0
            ));
        }
    }
    applied.changed |= !plan.steps.is_empty();
    for step in plan.steps {
        match step {
            Step::Copy { name, tree } => {
                let from = store.path(&tree).unwrap_or_default();
                let to = root.join(SKILLS_DIR).join(&name);
                copy_tree(&from, &to).map_err(|err| failed(&format!("copy {name}"), err))?;
                let stamps = stamps(&to).map_err(|err| failed("read the new copy", err))?;
                seen.copies.insert(name, (stamps, tree));
            }
            Step::RemoveCopy(name) => {
                seen.copies.remove(&name);
                clear(&root.join(SKILLS_DIR).join(&name))
                    .map_err(|err| failed(&format!("remove {name}"), err))?;
            }
            step => run(&step, None).map_err(|err| failed("link the skills", err))?,
        }
    }
    applied.clashes = plan.clashes;
    make_dir(&root.join(PROPOSALS_DIR), None)
        .map_err(|err| failed("make the proposals folder", err))?;
    let mut wanted = plan.exclude;
    if lock.pack.is_some() {
        wanted.extend(super::pack::exclude_lines(root));
    }
    applied.changed |=
        exclude(root, &wanted, None).map_err(|err| failed("update git's exclude file", err))?;
    let mirror = root.join(LOCK_MIRROR);
    let text = lock::mirror(lock);
    // A link here gets replaced below, so nothing reads through it.
    let same = fs::symlink_metadata(&mirror).is_ok_and(|meta| meta.is_file())
        && read_regular(&mirror, text.len() as u64).is_ok_and(|bytes| bytes == text.as_bytes());
    if !same {
        write_atomic(&mirror, text.as_bytes(), Atomic::PLAIN)
            .map_err(|err| failed("write the lock mirror", err))?;
        applied.changed = true;
    }
    Ok(applied)
}

/// The tree each copy in the checkout holds, as far as it's one MaxHQ
/// wrote. A copy whose tree MaxHQ never wrote is kept as drift and left
/// out, so the plan writes it again.
fn check_copies(
    store: &dyn TreeStore,
    root: &Path,
    lock: &SkillLock,
    seen: &mut Seen,
    applied: &mut Applied,
) -> io::Result<BTreeMap<String, TreeHash>> {
    let dir = root.join(SKILLS_DIR);
    let mut copies = BTreeMap::new();
    if !fs::symlink_metadata(&dir).is_ok_and(|meta| meta.is_dir()) {
        seen.copies.clear();
        return Ok(copies);
    }
    seen.copies
        .retain(|name, _| dir.join(name).symlink_metadata().is_ok());
    let mut names: Vec<String> = fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort_unstable();
    for name in names {
        let copy = dir.join(&name);
        let now = stamps(&copy)?;
        let locked = lock.skills.iter().find(|s| s.name == name).map(|s| &s.tree);
        let (tree, known) = match seen.copies.get(&name) {
            Some((was, tree)) if *was == now => (Some(tree.clone()), true),
            _ => {
                seen.hashed += 1;
                (tree::of(&DiskFs, &copy).ok(), false)
            }
        };
        let ours = tree
            .as_ref()
            .is_some_and(|tree| known || Some(tree) == locked || store.has(tree));
        match tree {
            Some(tree) if ours => {
                seen.copies.insert(name.clone(), (now, tree.clone()));
                copies.insert(name, tree);
            }
            tree => {
                seen.copies.remove(&name);
                let kept = keep(root, &name, tree.as_ref(), &copy)?;
                applied.changed = true;
                applied.drift.push(Drifted {
                    name,
                    base: locked.cloned(),
                    tree,
                    kept,
                });
            }
        }
    }
    Ok(copies)
}

/// Moves a changed copy into the proposals folder, under its name or,
/// when that's taken, its name and tree, and lets it be written again.
fn keep(root: &Path, name: &str, tree: Option<&TreeHash>, copy: &Path) -> io::Result<PathBuf> {
    let proposals = root.join(PROPOSALS_DIR);
    make_dir(&proposals, None)?;
    let short = tree.map_or("changed", |tree| &tree.0[..tree.0.len().min(12)]);
    let mut to = proposals.join(name);
    let mut n = 1;
    while to.symlink_metadata().is_ok() {
        to = proposals.join(format!("{name}-{short}-{n}"));
        n += 1;
    }
    writable(copy)?;
    fs::rename(copy, &to)?;
    set_modes(&to, false)?;
    Ok(to)
}

/// Every file and folder under `dir`, sorted, with what `stat` says of it.
fn stamps(dir: &Path) -> io::Result<Vec<Stamp>> {
    let mut found = Vec::new();
    let mut todo = vec![PathBuf::new()];
    while let Some(rel) = todo.pop() {
        for entry in fs::read_dir(dir.join(&rel))? {
            let entry = entry?;
            let path = rel.join(entry.file_name());
            let meta = entry.metadata()?;
            if meta.is_dir() {
                todo.push(path.clone());
            }
            found.push((
                path,
                meta.len(),
                meta.modified().ok(),
                fsmode::mode_of(&meta),
            ));
        }
    }
    found.sort_unstable();
    Ok(found)
}

/// Writes the tree at `from` to `to` through a folder beside it that's
/// renamed into place, read-only: 0444 files, 0555 executables and
/// folders. A copy at `to` goes. The top folder turns read-only only once
/// it's in place, since macOS 15 won't rename a read-only folder.
fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    let temp = beside(to, "tmp");
    clear(&temp)?;
    let copied = copy_dir(from, &temp)
        .and_then(|()| set_modes(&temp, true))
        .and_then(|()| fsmode::set_mode(&temp, 0o755));
    if let Err(err) = copied {
        let _ = clear(&temp);
        return Err(err);
    }
    let old = beside(to, "old");
    if to.symlink_metadata().is_ok() {
        clear(&old)?;
        writable(to)?;
        fs::rename(to, &old)?;
    }
    fs::rename(&temp, to)?;
    fsmode::set_mode(to, 0o555)?;
    clear(&old)
}

/// A path beside `path` for a temporary folder, hidden from the planner
/// by its leading dot.
pub(crate) fn beside(path: &Path, what: &str) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let n = TEMPS.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.{}-{n}.{what}", std::process::id()))
}

/// Copies the folder at `from` to `to`, files and folders only.
pub(crate) fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Sets every mode under `dir`, children first: read-only when
/// `read_only`, or back to writable for you. Executables stay executable.
/// Windows has no modes, so there it changes nothing.
pub(crate) fn set_modes(dir: &Path, read_only: bool) -> io::Result<()> {
    let (file, exec, folder) = if read_only {
        (0o444, 0o555, 0o555)
    } else {
        (0o644, 0o755, 0o755)
    };
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let meta = entry.metadata()?;
        if meta.is_dir() {
            set_modes(&path, read_only)?;
        } else if meta.is_file() {
            let mode = if fsmode::mode_of(&meta) & 0o111 != 0 {
                exec
            } else {
                file
            };
            fsmode::set_mode(&path, mode)?;
        }
    }
    fsmode::set_mode(dir, folder)
}

/// How deep [`writable`] goes, since it holds a folder open at each level.
const WRITABLE_DEPTH: usize = 256;

/// Lets every folder under `dir` be written, so it can be moved or
/// removed. Root does this to folders whose owner can swap any of them for
/// a link at any moment, so each mode changes through a folder held open,
/// and whatever a link names keeps its mode.
pub(crate) fn writable(dir: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(dir)?.is_dir() {
        return Ok(());
    }
    writable_at(dir, &|| Dir::open(dir), 0)
}

/// [`writable`] for the folder at `path` that `open` opens, `depth` levels
/// down.
fn writable_at(path: &Path, open: &dyn Fn() -> io::Result<Dir>, depth: usize) -> io::Result<()> {
    if depth > WRITABLE_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("it holds folders over {WRITABLE_DEPTH} deep"),
        ));
    }
    let folder = match open() {
        Ok(folder) => folder,
        // The name stopped being a folder after it was checked.
        Err(err) if fs_ext::not_a_folder_now(&err) => return Ok(()),
        // A user can own a folder it can't read. Root can read them all, so
        // it never reaches this change by name, which would follow a link.
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied && !maxhq_os::proc::is_root() => {
            fsmode::set_mode(path, 0o755)?;
            open()?
        }
        Err(err) => return Err(err),
    };
    folder.set_own_mode(0o755)?;
    for name in folder.raw_names()? {
        // `Dir` takes names as text, and root doesn't need the mode to remove
        // what's under a name that isn't.
        let Some(name) = name.to_str() else {
            continue;
        };
        if folder.kind(name)? == Some(FileType::Directory) {
            let parts = [name.to_owned()];
            writable_at(&path.join(name), &|| folder.dir(&parts), depth + 1)?;
        }
    }
    Ok(())
}

/// Removes what's at `path`, a read-only copy too. Nothing there is fine.
pub(crate) fn clear(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
        Ok(meta) if meta.is_dir() => {
            writable(path)?;
            maxhq_transfer::remove_tree(path)
        }
        Ok(_) => fs_ext::remove_link(path),
    }
}

/// Makes a folder and the parents it lacks, owned by `owner` when given.
pub(crate) fn make_dir(dir: &Path, owner: Option<(u32, u32)>) -> io::Result<()> {
    let mut missing = Vec::new();
    let mut at = Some(dir);
    while let Some(path) = at
        && path.symlink_metadata().is_err()
    {
        missing.push(path.to_owned());
        at = path.parent();
    }
    fs::create_dir_all(dir)?;
    for path in missing.iter().rev() {
        own(path, owner)?;
    }
    Ok(())
}

/// Gives what's at `path` to `owner`, when there is one.
pub(crate) fn own(path: &Path, owner: Option<(u32, u32)>) -> io::Result<()> {
    match owner {
        Some((uid, gid)) => fs_ext::chown_link(path, uid, gid),
        None => Ok(()),
    }
}

/// Makes one of a plan's folder and link steps. What it makes goes to
/// `owner` when given, as in a container, where the guest runs as root.
pub(crate) fn run(step: &Step, owner: Option<(u32, u32)>) -> io::Result<()> {
    match step {
        Step::MakeDir(dir) => make_dir(dir, owner),
        Step::Clear(path) => clear(path),
        Step::Link { path, target } => {
            if let Some(parent) = path.parent() {
                make_dir(parent, owner)?;
            }
            fs_ext::symlink(target, path)?;
            own(path, owner)
        }
        Step::Relink { path, target } => {
            let temp = beside(path, "link");
            fs_ext::symlink(target, &temp)?;
            own(&temp, owner)?;
            fs_ext::rename_link(&temp, path)
        }
        Step::Unlink(path) => fs_ext::remove_link(path),
        Step::Copy { .. } | Step::RemoveCopy(_) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "copies are made from the store",
        )),
    }
}

/// Puts `wanted` in the exclude file of the repo at `root`, which linked
/// worktrees share, and takes out the lines MaxHQ added that `wanted` no
/// longer has. Says whether the file changed. A folder that isn't in a
/// repo has nothing to exclude.
pub(crate) fn exclude(
    root: &Path,
    wanted: &[String],
    owner: Option<(u32, u32)>,
) -> io::Result<bool> {
    let Some(git) = common_dir(root) else {
        return Ok(false);
    };
    let info = git.join("info");
    let path = info.join("exclude");
    let existing = match read_text(&path, MAX_GIT_FILE) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    let kept = stale_out(&existing, wanted);
    let text = plan::merge_exclude(&kept, wanted).unwrap_or(kept);
    if text == existing {
        return Ok(false);
    }
    make_dir(&info, owner)?;
    write_atomic(&path, text.as_bytes(), Atomic::PLAIN)?;
    own(&path, owner)?;
    Ok(true)
}

/// Takes `lines` out of the exclude file of the repo at `root` where
/// [`exclude`] put them, leaving every other line. Says whether the file
/// changed.
pub(crate) fn unexclude(
    root: &Path,
    lines: &[String],
    owner: Option<(u32, u32)>,
) -> io::Result<bool> {
    let Some(git) = common_dir(root) else {
        return Ok(false);
    };
    let path = git.join("info").join("exclude");
    let existing = match read_text(&path, MAX_GIT_FILE) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    let Some(text) = plan::unmerge_exclude(&existing, lines) else {
        return Ok(false);
    };
    write_atomic(&path, text.as_bytes(), Atomic::PLAIN)?;
    own(&path, owner)?;
    Ok(true)
}

/// `text` without the lines MaxHQ adds that `wanted` lacks: all of
/// `.maxhq`, its proposals, and a link to one skill. The heading goes once
/// no line is left under it.
fn stale_out(text: &str, wanted: &[String]) -> String {
    let ours = |line: &str| {
        line == "/.maxhq/"
            || line == format!("/{PROPOSALS_DIR}/")
            || LINK_DIRS.iter().any(|dir| {
                line.strip_prefix(&format!("/{dir}/"))
                    .is_some_and(|name| !name.is_empty() && !name.contains('/'))
            })
    };
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| {
            let line = line.trim();
            !ours(line) || wanted.iter().any(|w| w == line)
        })
        .collect();
    let heading_alone = !lines.iter().any(|line| ours(line.trim()));
    let mut out = String::new();
    for line in lines {
        if heading_alone && line.trim() == EXCLUDE_HEADING {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The most this reads of a file in a git folder. Those it reads are a few
/// lines long.
const MAX_GIT_FILE: u64 = 16 * 1024 * 1024;

/// The git folder that the checkout at `root` shares with its linked
/// worktrees: `.git` itself, or the folder its `gitdir` and `commondir`
/// name.
fn common_dir(root: &Path) -> Option<PathBuf> {
    let dot = root.join(".git");
    if fs::symlink_metadata(&dot).ok()?.is_dir() {
        return Some(dot);
    }
    let text = read_text(&dot, MAX_GIT_FILE).ok()?;
    let dir = root.join(text.trim().strip_prefix("gitdir:")?.trim());
    let common = match read_text(&dir.join("commondir"), MAX_GIT_FILE) {
        Ok(common) => dir.join(common.trim()),
        Err(_) => dir,
    };
    // A `.git` file can name any path, so this takes only what git would.
    let real = common.join("HEAD").is_file()
        && common.join("objects").is_dir()
        && common.join("refs").is_dir();
    real.then_some(common)
}

/// The most [`rewrite_as`] reads of an exclude file.
const MAX_EXCLUDE: u64 = 16 * 1024 * 1024;

/// Puts `wanted` in the exclude file of the repo at `root`, as [`exclude`]
/// does, for a caller running as root for `owner`. See [`rewrite_as`].
pub(crate) fn exclude_as(
    work: &Path,
    root: &Path,
    wanted: &[String],
    owner: (u32, u32),
) -> io::Result<bool> {
    rewrite_as(work, root, owner, |existing| {
        let kept = stale_out(existing, wanted);
        Some(plan::merge_exclude(&kept, wanted).unwrap_or(kept))
    })
}

/// Takes `lines` out of the exclude file of the repo at `root`, as
/// [`unexclude`] does, for a caller running as root for `owner`. See
/// [`rewrite_as`].
pub(crate) fn unexclude_as(
    work: &Path,
    root: &Path,
    lines: &[String],
    owner: (u32, u32),
) -> io::Result<bool> {
    rewrite_as(work, root, owner, |existing| {
        plan::unmerge_exclude(existing, lines)
    })
}

/// Opens the git folder of the checkout at `root`, none when it isn't in a
/// repo. `owner` can aim `.git`, `gitdir` and `commondir` anywhere, so the
/// folder must really be under `work`, and it opens from there without
/// following a link.
pub(crate) fn git_folder_in(work: &Path, root: &Path) -> io::Result<Option<Dir>> {
    let Some(git) = common_dir(root) else {
        return Ok(None);
    };
    let outside = || {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the checkout's git folder isn't in the work folder",
        )
    };
    let work = fs::canonicalize(work)?;
    let git = fs::canonicalize(git).map_err(|_| outside())?;
    let parts = git
        .strip_prefix(&work)
        .map_err(|_| outside())?
        .iter()
        .map(|part| part.to_str().map(str::to_owned))
        .collect::<Option<Vec<String>>>()
        .ok_or_else(outside)?;
    Dir::open(&work)?.dir(&parts).map(Some)
}

/// Changes the exclude file of the repo at `root` as `edit` says, for a
/// caller running as root for `owner`, who can swap any of it for a link
/// at any moment. So every read and write goes through a folder that's
/// open already, and a link in the way fails.
fn rewrite_as(
    work: &Path,
    root: &Path,
    (uid, gid): (u32, u32),
    edit: impl FnOnce(&str) -> Option<String>,
) -> io::Result<bool> {
    let Some(git) = git_folder_in(work, root)? else {
        return Ok(false);
    };
    let info = ["info".to_owned()];
    let existing = match git.kind("info")? {
        Some(_) => read_exclude(&git.dir(&info)?)?,
        None => String::new(),
    };
    let Some(text) = edit(&existing).filter(|text| *text != existing) else {
        return Ok(false);
    };
    if git.kind("info")?.is_none() {
        git.make_dir("info", 0o755)?;
        git.chown("info", uid, gid)?;
    }
    let info = git.dir(&info)?;
    let n = TEMPS.fetch_add(1, Ordering::Relaxed);
    let temp = format!(".exclude.{}-{n}.tmp", std::process::id());
    let mut file = info.open_file(&temp, true)?;
    check_one_name(&file)?;
    file.set_len(0)?;
    file.write_all(text.as_bytes())?;
    set_mode(&file, false)?;
    fs_ext::chown_file(&file, uid, gid)?;
    info.rename(&temp, "exclude")?;
    Ok(true)
}

/// The exclude file in the open folder `info`, empty when there's none.
fn read_exclude(info: &Dir) -> io::Result<String> {
    match info.kind("exclude")? {
        Some(FileType::RegularFile) => {}
        None => return Ok(String::new()),
        Some(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the exclude file isn't a regular file",
            ))
        }
    }
    let file = info.read_file("exclude")?;
    check_one_name(&file)?;
    let mut text = String::new();
    file.take(MAX_EXCLUDE + 1).read_to_string(&mut text)?;
    if text.len() as u64 > MAX_EXCLUDE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the exclude file is too large",
        ));
    }
    Ok(text)
}

/// Refuses a file with a second name, which could be someone else's.
fn check_one_name(file: &fs::File) -> io::Result<()> {
    if fs_ext::has_other_names(file)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the exclude file has another name",
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use maxhq_core::fs_ext::symlink;
    use maxhq_core::skills::store::FolderStore;
    use maxhq_protocol::ids::SkillSourceId;
    use maxhq_protocol::skills::LockedSkill;

    use super::*;
    use crate::testing::TempDir;

    /// A store with one tree per `(name, body)`, and the lock of them.
    pub(crate) fn stocked(dir: &Path, skills: &[(&str, &str)]) -> (FolderStore, SkillLock) {
        let store = FolderStore::new(dir.join("store"));
        let mut lock = SkillLock {
            skills: Vec::new(),
            committed: false,
            pack: None,
        };
        for (name, body) in skills {
            let tree = land(&store, dir, name, body);
            lock::put(
                &mut lock,
                LockedSkill {
                    name: (*name).into(),
                    source: SkillSourceId::derive("example/agent-skills"),
                    path: format!("skills/{name}"),
                    commit: None,
                    tree,
                    pinned: false,
                },
            );
        }
        (store, lock)
    }

    /// Lands a skill with `body` in `store` and gives its tree.
    pub(crate) fn land(store: &FolderStore, dir: &Path, name: &str, body: &str) -> TreeHash {
        let from = dir.join(format!(
            "in-{name}-{}",
            TEMPS.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(from.join("scripts")).unwrap();
        fs::write(
            from.join("SKILL.md"),
            format!("---\nname: {name}\n---\n{body}"),
        )
        .unwrap();
        let script = from.join("scripts/run.sh");
        fs::write(&script, "#!/bin/sh\n").unwrap();
        fsmode::set_mode(&script, 0o755).unwrap();
        let tree = tree::of(&DiskFs, &from).unwrap();
        store.land(&tree, &from).unwrap();
        tree
    }

    /// A repo at `<dir>/repo` with a `.git` folder.
    fn repo(dir: &Path) -> PathBuf {
        let root = dir.join("repo");
        fs::create_dir_all(root.join(".git/info")).unwrap();
        root
    }

    /// Checks that `path`'s own mode is `want`. Windows has no modes.
    fn assert_mode(path: &Path, want: u32) {
        if cfg!(unix) {
            let mode = fsmode::mode_of(&fs::symlink_metadata(path).unwrap()) & 0o777;
            assert_eq!(mode, want, "{}", path.display());
        }
    }

    fn exclude_file(root: &Path) -> String {
        fs::read_to_string(root.join(".git/info/exclude")).unwrap_or_default()
    }

    #[test]
    fn copies_land_read_only_with_relative_links_and_excluded() {
        let dir = TempDir::new();
        let (store, lock) = stocked(dir.path(), &[("git-flow", "one")]);
        let root = repo(dir.path());
        let applied = checkout(&store, &root, &lock, &mut Seen::default()).unwrap();
        assert!(applied.changed && applied.drift.is_empty());

        let copy = root.join(".maxhq/skills/git-flow");
        assert_mode(&copy, 0o555);
        assert_mode(&copy.join("SKILL.md"), 0o444);
        assert_mode(&copy.join("scripts/run.sh"), 0o555);
        assert_eq!(tree::of(&DiskFs, &copy).unwrap(), lock.skills[0].tree);
        for link in LINK_DIRS {
            let path = root.join(link).join("git-flow");
            assert_eq!(
                fs_ext::read_link(&path).unwrap(),
                Path::new("../../.maxhq/skills/git-flow")
            );
            assert!(path.join("SKILL.md").is_file());
        }
        assert!(root.join(".maxhq/proposals").is_dir());
        assert_eq!(
            fs::read_to_string(root.join(LOCK_MIRROR)).unwrap(),
            lock::mirror(&lock)
        );
        assert_eq!(
            exclude_file(&root),
            "# MaxHQ skills\n/.maxhq/\n/.claude/skills/git-flow\n/.agents/skills/git-flow\n"
        );
    }

    #[test]
    fn a_clean_check_hashes_nothing_and_changes_nothing() {
        let dir = TempDir::new();
        let (store, lock) = stocked(dir.path(), &[("git-flow", "one"), ("unslop", "two")]);
        let root = repo(dir.path());
        let mut seen = Seen::default();
        checkout(&store, &root, &lock, &mut seen).unwrap();
        let hashed = seen.hashed;

        let again = checkout(&store, &root, &lock, &mut seen).unwrap();
        assert!(!again.changed);
        assert_eq!(seen.hashed, hashed);

        // A new run knows nothing, so it hashes each copy once.
        let mut fresh = Seen::default();
        assert!(!checkout(&store, &root, &lock, &mut fresh).unwrap().changed);
        assert_eq!(fresh.hashed, 2);
    }

    #[test]
    fn exclude_lines_are_added_once_and_shared_by_a_linked_worktree() {
        let dir = TempDir::new();
        let (store, lock) = stocked(dir.path(), &[("git-flow", "one")]);
        let root = repo(dir.path());
        fs::write(root.join(".git/info/exclude"), "*.log\n").unwrap();
        // A linked worktree's `.git` is a file naming its own folder, whose
        // `commondir` leads back to the repo's.
        let linked = dir.path().join("linked");
        let own = root.join(".git/worktrees/linked");
        fs::create_dir_all(&own).unwrap();
        fs::write(own.join("commondir"), "../..\n").unwrap();
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::create_dir_all(root.join(".git/refs")).unwrap();
        fs::create_dir_all(&linked).unwrap();
        fs::write(linked.join(".git"), format!("gitdir: {}\n", own.display())).unwrap();

        checkout(&store, &root, &lock, &mut Seen::default()).unwrap();
        checkout(&store, &linked, &lock, &mut Seen::default()).unwrap();
        checkout(&store, &root, &lock, &mut Seen::default()).unwrap();
        assert_eq!(
            exclude_file(&root),
            "*.log\n# MaxHQ skills\n/.maxhq/\n/.claude/skills/git-flow\n/.agents/skills/git-flow\n"
        );
        assert!(!own.join("info/exclude").exists());
    }

    #[test]
    fn exclude_lines_reach_a_real_worktree_and_submodule_and_no_other_folder() {
        let dir = TempDir::new();
        let git = |at: &Path, args: &[&str]| {
            let done = crate::guest::worktree::git(None)
                .current_dir(at)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(["-c", "protocol.file.allow=always"])
                .args(args)
                .output()
                .unwrap();
            let why = String::from_utf8_lossy(&done.stderr);
            assert!(done.status.success(), "{args:?}: {why}");
        };
        let main = dir.path().join("main");
        let other = dir.path().join("other");
        for at in [&main, &other] {
            fs::create_dir_all(at).unwrap();
            git(at, &["init", "-q"]);
            git(at, &["commit", "-q", "--allow-empty", "-m", "first"]);
        }
        let linked = dir.path().join("linked");
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "side",
                linked.to_str().unwrap(),
            ],
        );
        git(
            &main,
            &["submodule", "add", "-q", other.to_str().unwrap(), "inner"],
        );
        let wanted = ["/.maxhq/".to_owned()];

        assert!(exclude(&linked, &wanted, None).unwrap());
        let shared = fs::read_to_string(main.join(".git/info/exclude")).unwrap();
        assert!(shared.contains("/.maxhq/"), "{shared}");
        assert!(exclude(&main.join("inner"), &wanted, None).unwrap());
        let inner = fs::read_to_string(main.join(".git/modules/inner/info/exclude")).unwrap();
        assert!(inner.contains("/.maxhq/"), "{inner}");

        // A `.git` file naming a plain folder, directly or through `commondir`.
        let far = dir.path().join("far");
        let by_way = dir.path().join("by-way");
        fs::create_dir_all(&far).unwrap();
        fs::create_dir_all(&by_way).unwrap();
        fs::write(by_way.join("commondir"), format!("{}\n", far.display())).unwrap();
        let hostile = dir.path().join("hostile");
        fs::create_dir_all(&hostile).unwrap();
        for named in [&far, &by_way] {
            let text = format!("gitdir: {}\n", named.display());
            fs::write(hostile.join(".git"), text).unwrap();
            assert!(!exclude(&hostile, &wanted, None).unwrap());
            assert!(!unexclude(&hostile, &[], None).unwrap());
            assert!(!far.join("info").exists());
        }
    }

    // Only Unix runs maxd as root for a container's user.
    #[cfg(unix)]
    #[test]
    fn taking_lines_out_as_root_stays_off_a_link_to_someone_elses_file() {
        use std::os::unix::fs::MetadataExt;

        let dir = TempDir::new();
        let root = repo(dir.path());
        let mine = fs::metadata(&root).unwrap();
        let owner = (mine.uid(), mine.gid());
        let lines = ["/.devflow-vendor/".to_owned()];
        let other = dir.path().join("other");
        fs::write(
            &other,
            "# MaxHQ skills\n/.devflow-vendor/\nnot the guest's\n",
        )
        .unwrap();
        let exclude = root.join(".git/info/exclude");
        symlink(&other, &exclude).unwrap();

        assert!(unexclude_as(dir.path(), &root, &lines, owner).is_err());
        assert!(fs::symlink_metadata(&exclude).unwrap().is_symlink());

        fs::remove_file(&exclude).unwrap();
        fs::write(&exclude, "*.log\n# MaxHQ skills\n/.devflow-vendor/\n").unwrap();
        assert!(unexclude_as(dir.path(), &root, &lines, owner).unwrap());
        assert_eq!(exclude_file(&root), "*.log\n");
    }

    #[test]
    fn committing_copies_excludes_only_the_proposals() {
        let dir = TempDir::new();
        let (store, mut lock) = stocked(dir.path(), &[("git-flow", "one")]);
        let root = repo(dir.path());
        fs::write(root.join(".git/info/exclude"), "*.log\n").unwrap();
        checkout(&store, &root, &lock, &mut Seen::default()).unwrap();

        lock.committed = true;
        checkout(&store, &root, &lock, &mut Seen::default()).unwrap();
        assert_eq!(
            exclude_file(&root),
            "*.log\n# MaxHQ skills\n/.maxhq/proposals/\n"
        );

        lock.committed = false;
        lock.skills.clear();
        checkout(&store, &root, &lock, &mut Seen::default()).unwrap();
        assert_eq!(exclude_file(&root), "*.log\n# MaxHQ skills\n/.maxhq/\n");
    }

    #[test]
    fn a_changed_copy_is_kept_as_a_proposal_and_the_lock_comes_back() {
        let dir = TempDir::new();
        let (store, lock) = stocked(dir.path(), &[("git-flow", "one")]);
        let root = repo(dir.path());
        let mut seen = Seen::default();
        checkout(&store, &root, &lock, &mut seen).unwrap();

        let copy = root.join(".maxhq/skills/git-flow");
        fsmode::set_mode(&copy, 0o755).unwrap();
        fsmode::set_mode(&copy.join("SKILL.md"), 0o644).unwrap();
        fs::write(copy.join("SKILL.md"), "---\nname: git-flow\n---\nbetter").unwrap();
        // A proposal of that name is there already, so this one goes beside it.
        fs::create_dir_all(root.join(".maxhq/proposals/git-flow")).unwrap();

        let applied = checkout(&store, &root, &lock, &mut seen).unwrap();
        assert_eq!(applied.drift.len(), 1);
        let drifted = &applied.drift[0];
        assert_eq!(drifted.name, "git-flow");
        assert_eq!(drifted.base.as_ref(), Some(&lock.skills[0].tree));
        let kept = &drifted.kept;
        assert!(
            kept.starts_with(root.join(".maxhq/proposals"))
                && kept != &root.join(".maxhq/proposals/git-flow")
        );
        assert_eq!(
            fs::read_to_string(kept.join("SKILL.md")).unwrap(),
            "---\nname: git-flow\n---\nbetter"
        );
        assert_eq!(drifted.tree, Some(tree::of(&DiskFs, kept).unwrap()));
        assert_mode(&kept.join("SKILL.md"), 0o644);
        assert_eq!(tree::of(&DiskFs, &copy).unwrap(), lock.skills[0].tree);
        assert_mode(&copy, 0o555);
    }

    #[test]
    fn an_older_copy_from_the_store_is_updated_not_kept() {
        let dir = TempDir::new();
        let (store, old) = stocked(dir.path(), &[("git-flow", "one")]);
        let root = repo(dir.path());
        checkout(&store, &root, &old, &mut Seen::default()).unwrap();

        let mut new = old.clone();
        new.skills[0].tree = land(&store, dir.path(), "git-flow", "two");
        // A new run, so nothing is known of the old copy but its tree.
        let applied = checkout(&store, &root, &new, &mut Seen::default()).unwrap();
        assert!(applied.changed && applied.drift.is_empty());
        assert_eq!(
            tree::of(&DiskFs, &root.join(".maxhq/skills/git-flow")).unwrap(),
            new.skills[0].tree
        );
    }

    #[test]
    fn a_removed_skill_loses_its_copy_and_links_and_others_stay() {
        let dir = TempDir::new();
        let (store, mut lock) = stocked(dir.path(), &[("git-flow", "one"), ("unslop", "two")]);
        let root = repo(dir.path());
        // A skill the project commits itself, and a link devflow made.
        fs::create_dir_all(root.join(".claude/skills/mine")).unwrap();
        fs::create_dir_all(root.join(".agents/skills")).unwrap();
        symlink(
            "/opt/devflow/skills/unslop",
            root.join(".agents/skills/unslop"),
        )
        .unwrap();
        let mut seen = Seen::default();
        let applied = checkout(&store, &root, &lock, &mut seen).unwrap();
        assert_eq!(applied.clashes.len(), 1);

        lock::remove(&mut lock, "git-flow");
        checkout(&store, &root, &lock, &mut seen).unwrap();
        assert!(!root.join(".maxhq/skills/git-flow").exists());
        assert!(root
            .join(".claude/skills/git-flow")
            .symlink_metadata()
            .is_err());
        assert!(root.join(".claude/skills/mine").is_dir());
        assert_eq!(
            fs_ext::read_link(root.join(".agents/skills/unslop")).unwrap(),
            Path::new("/opt/devflow/skills/unslop")
        );
    }

    /// Everything under `root`, sorted, with each file's text and each
    /// link's target.
    fn listing(root: &Path) -> Vec<String> {
        let mut found = Vec::new();
        let mut todo = vec![root.to_owned()];
        while let Some(dir) = todo.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let shown = path.strip_prefix(root).unwrap().display().to_string();
                let kind = entry.file_type().unwrap();
                if kind.is_symlink() {
                    let target = fs::read_link(&path).unwrap();
                    found.push(format!("{shown} -> {}", target.display()));
                } else if kind.is_dir() {
                    found.push(format!("{shown}/"));
                    todo.push(path);
                } else {
                    found.push(format!("{shown}: {}", fs::read_to_string(&path).unwrap()));
                }
            }
        }
        found.sort();
        found
    }

    /// A home with skills of the owner's own, which nothing may touch.
    fn owners_home(dir: &Path) -> PathBuf {
        let home = dir.join("home");
        let skills = home.join(".claude/skills");
        fs::create_dir_all(skills.join("mine")).unwrap();
        fs::write(skills.join("mine/SKILL.md"), "the owner's skill").unwrap();
        fs::write(skills.join("loose-file.txt"), "the owner's note").unwrap();
        // The folder comes first, so Windows makes a folder's link to it.
        fs::create_dir_all(home.join(".agents/skills/shared")).unwrap();
        symlink("../../.agents/skills/shared", skills.join("shared")).unwrap();
        home
    }

    #[test]
    fn a_linked_maxhq_folder_stops_the_checkout_and_the_home_stays_as_it_was() {
        let dir = TempDir::new();
        let (store, lock) = stocked(dir.path(), &[("git-flow", "one")]);
        let root = repo(dir.path());
        let home = owners_home(dir.path());
        let before = listing(&home);
        symlink(home.join(".claude"), root.join(".maxhq")).unwrap();

        let applied = checkout(&store, &root, &lock, &mut Seen::default()).unwrap();
        assert_eq!(applied.outside(&root), [".maxhq"]);
        assert!(!applied.changed && applied.drift.is_empty());
        assert_eq!(listing(&home), before);
        assert!(!root.join(".claude").exists() && !root.join(".agents").exists());
        assert!(!root.join(".git/info/exclude").exists());
    }

    #[test]
    fn a_linked_proposals_folder_gets_nothing_moved_through_it() {
        let dir = TempDir::new();
        let (store, lock) = stocked(dir.path(), &[("git-flow", "one")]);
        let root = repo(dir.path());
        let home = owners_home(dir.path());
        let before = listing(&home);
        // A folder the repo commits, which a check would keep as a proposal.
        fs::create_dir_all(root.join(".maxhq/skills/planted")).unwrap();
        fs::write(root.join(".maxhq/skills/planted/SKILL.md"), "planted").unwrap();
        symlink(home.join(".claude/skills"), root.join(".maxhq/proposals")).unwrap();

        let applied = checkout(&store, &root, &lock, &mut Seen::default()).unwrap();
        assert_eq!(applied.outside(&root), [".maxhq/proposals"]);
        assert!(!applied.changed && applied.drift.is_empty());
        assert_eq!(listing(&home), before);
        assert!(root.join(".maxhq/skills/planted/SKILL.md").is_file());
    }

    /// Runs `work` on its own thread and fails if it doesn't end soon.
    #[cfg(unix)]
    fn within<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .expect("it's still waiting on the file")
    }

    // Windows has no devices to link to by path.
    #[cfg(unix)]
    #[test]
    fn a_lock_mirror_linked_to_a_device_is_replaced_and_doesnt_hold_up_the_checkout() {
        for target in ["/dev/ptmx", "/dev/zero"] {
            let dir = TempDir::new();
            let (store, lock) = stocked(dir.path(), &[("git-flow", "one")]);
            let root = repo(dir.path());
            fs::create_dir_all(root.join(".maxhq")).unwrap();
            symlink(target, root.join(LOCK_MIRROR)).unwrap();
            let at = root.clone();
            let done = within(move || checkout(&store, &at, &lock, &mut Seen::default()).is_ok());
            assert!(done, "{target}");
            let mirror = fs::symlink_metadata(root.join(LOCK_MIRROR)).unwrap();
            assert!(mirror.is_file(), "{target}");
        }
    }

    #[test]
    fn a_lock_mirror_linked_to_the_right_text_is_still_replaced() {
        let dir = TempDir::new();
        let (store, lock) = stocked(dir.path(), &[("git-flow", "one")]);
        let root = repo(dir.path());
        let elsewhere = dir.path().join("elsewhere.json");
        fs::write(&elsewhere, lock::mirror(&lock)).unwrap();
        fs::create_dir_all(root.join(".maxhq")).unwrap();
        symlink(&elsewhere, root.join(LOCK_MIRROR)).unwrap();
        checkout(&store, &root, &lock, &mut Seen::default()).unwrap();
        assert!(fs::symlink_metadata(root.join(LOCK_MIRROR))
            .unwrap()
            .is_file());
    }

    #[cfg(unix)]
    #[test]
    fn an_exclude_file_that_is_a_pipe_fails_and_doesnt_hold_up_the_checkout() {
        let dir = TempDir::new();
        let root = dir.path().join("repo");
        fs::create_dir_all(root.join(".git/info")).unwrap();
        let made = std::process::Command::new("mkfifo")
            .arg(root.join(".git/info/exclude"))
            .status();
        assert!(made.unwrap().success());
        let failed = within(move || exclude(&root, &["line".to_owned()], None).is_err());
        assert!(failed);
    }

    #[cfg(unix)]
    #[test]
    fn a_git_file_linked_to_a_terminal_doesnt_hold_up_the_checkout() {
        let dir = TempDir::new();
        let root = dir.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        symlink("/dev/ptmx", root.join(".git")).unwrap();
        let changed = within(move || exclude(&root, &["line".to_owned()], None).unwrap());
        assert!(!changed);
    }

    #[test]
    fn a_cli_folder_linked_outside_gets_nothing_and_the_rest_lands() {
        for link in [".claude", ".claude/skills"] {
            let dir = TempDir::new();
            let (store, lock) = stocked(dir.path(), &[("git-flow", "one")]);
            let root = repo(dir.path());
            let home = owners_home(dir.path());
            let before = listing(&home);
            fs::create_dir_all(root.join(link).parent().unwrap()).unwrap();
            symlink(home.join(link), root.join(link)).unwrap();

            let applied = checkout(&store, &root, &lock, &mut Seen::default()).unwrap();
            assert_eq!(applied.outside(&root), [link]);
            assert_eq!(listing(&home), before, "{link}");
            assert!(root.join(".maxhq/skills/git-flow/SKILL.md").is_file());
            assert!(root.join(".maxhq/proposals").is_dir());
            assert_eq!(
                fs_ext::read_link(root.join(".agents/skills/git-flow")).unwrap(),
                Path::new("../../.maxhq/skills/git-flow")
            );
        }
    }

    #[test]
    fn a_cli_folder_linked_inside_the_checkout_still_gets_its_links() {
        let dir = TempDir::new();
        let (store, lock) = stocked(dir.path(), &[("git-flow", "one")]);
        let root = repo(dir.path());
        fs::create_dir_all(root.join("shared")).unwrap();
        symlink("shared", root.join(".claude")).unwrap();

        let applied = checkout(&store, &root, &lock, &mut Seen::default()).unwrap();
        assert!(applied.outside(&root).is_empty());
        assert_eq!(
            fs_ext::read_link(root.join("shared/skills/git-flow")).unwrap(),
            Path::new("../../.maxhq/skills/git-flow")
        );
    }

    #[test]
    fn a_cli_folder_linked_into_git_gets_nothing() {
        let dir = TempDir::new();
        let (store, lock) = stocked(dir.path(), &[("git-flow", "one")]);
        let root = repo(dir.path());
        fs::create_dir_all(root.join(".git/hooks")).unwrap();
        symlink(".git/hooks", root.join(".claude")).unwrap();

        let applied = checkout(&store, &root, &lock, &mut Seen::default()).unwrap();
        assert_eq!(applied.outside(&root), [".claude"]);
        assert_eq!(fs::read_dir(root.join(".git/hooks")).unwrap().count(), 0);
    }

    #[test]
    fn a_tree_the_store_lacks_is_refused_before_anything_changes() {
        let dir = TempDir::new();
        let (store, mut lock) = stocked(dir.path(), &[("git-flow", "one")]);
        lock.skills[0].tree = TreeHash("a".repeat(64));
        let root = repo(dir.path());
        let err = checkout(&store, &root, &lock, &mut Seen::default()).unwrap_err();
        assert!(err.contains("doesn't have the skill tree"));
        assert!(!root.join(".maxhq").exists());
    }

    #[test]
    fn stale_lines_go_and_yours_stay() {
        let text = "*.log\n# MaxHQ skills\n/.maxhq/\n/.claude/skills/old\n/.claude/skills/a/b\n";
        assert_eq!(
            stale_out(text, &["/.maxhq/".into()]),
            "*.log\n# MaxHQ skills\n/.maxhq/\n/.claude/skills/a/b\n"
        );
        assert_eq!(stale_out("# MaxHQ skills\n/.maxhq/\n", &[]), "");
    }

    // It swaps a folder and a link at once, which only Linux and macOS can.
    #[cfg(unix)]
    #[test]
    fn making_a_tree_writable_leaves_a_file_a_swapped_link_names() {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        use rustix::fs::{renameat_with, RenameFlags, CWD};

        let mode_of = |path: &Path| fsmode::mode_of(&fs::metadata(path).unwrap()) & 0o777;

        let dir = TempDir::new();
        let tree = dir.path().join("tree");
        let sub = tree.join("sub");
        fs::create_dir_all(&sub).unwrap();
        let victim = dir.path().join("victim");
        fs::write(&victim, "secret").unwrap();
        fsmode::set_mode(&victim, 0o600).unwrap();
        let link = dir.path().join("link");
        symlink(&victim, &link).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let swapping = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let _ = renameat_with(CWD, &sub, CWD, &link, RenameFlags::EXCHANGE);
                }
            })
        };
        let end = Instant::now() + Duration::from_secs(1);
        while Instant::now() < end && mode_of(&victim) == 0o600 {
            let _ = writable(&tree);
        }
        stop.store(true, Ordering::Relaxed);
        swapping.join().unwrap();
        assert_eq!(mode_of(&victim), 0o600);
    }

    #[test]
    fn a_folder_its_owner_cant_read_can_still_be_cleared() {
        let dir = TempDir::new();
        let tree = dir.path().join("tree");
        let shut = tree.join("shut");
        fs::create_dir_all(&shut).unwrap();
        fs::write(shut.join("file"), "x").unwrap();
        fsmode::set_mode(&shut, 0o000).unwrap();
        clear(&tree).unwrap();
        assert!(!tree.exists());
    }

    #[test]
    fn making_a_tree_writable_stops_at_folders_too_deep() {
        let dir = TempDir::new();
        let tree = dir.path().join("tree");
        let deep = (0..=WRITABLE_DEPTH + 1).fold(tree.clone(), |at, _| at.join("d"));
        fs::create_dir_all(&deep).unwrap();
        let refused = writable(&tree).unwrap_err();
        assert!(refused.to_string().contains("deep"), "{refused}");
    }
}
