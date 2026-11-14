// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! W-4: a worktree on a new branch for an agent, made by
//! `git worktree add` as user `agent`.
//!
//! The repo is the folder under the work folder that the workspace's repo
//! URL names, or the only repo there. Worktrees go in `.worktrees` beside
//! it, one folder per branch.
//!
//! Listing the files git ignores in a checkout, for copying a project, runs
//! as `agent` too.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use maxhq_core::ignored_files::{self, Ignored};
use maxhq_core::skills::disk::DiskFs;
use maxhq_core::skills::frontmatter::check_name as check_skill;
use maxhq_core::skills::lock as skill_lock;
use maxhq_core::skills::pack::PACK_DIR;
use maxhq_core::skills::plan::{self, Clash, Layout, Step};
use maxhq_core::skills::{tree, Node, SkillFs, MOUNT, PROPOSALS_DIR};
use maxhq_hold::RunAs;
use maxhq_protocol::guest::{GuestProposal, GuestProposalFile, Worktree, MAX_PROPOSAL_BYTES};
use maxhq_protocol::ids::SkillSourceId;
use maxhq_protocol::message::EntryKind;
use maxhq_protocol::skills::{LockedSkill, PackState, SkillLock, TreeHash};
use maxhq_transfer::beneath::{Dir, FileType};
use tokio::process::Command;

use crate::skills::{apply, pack};
use crate::transfer::disk;

/// Where worktrees go, under the work folder.
pub(crate) const WORKTREES: &str = ".worktrees";

/// How long fetching one branch for a new worktree may take.
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// The worktree folder for `branch`, under the work folder.
pub(crate) fn path(work: &Path, branch: &str) -> PathBuf {
    work.join(WORKTREES).join(branch.replace('/', "-"))
}

/// The path git runs with when it runs as `agent`, and an SSH channel's
/// process starts with.
pub(crate) const PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Makes the worktree for `branch` from `base`, or from the repo's default
/// branch, running git as `user`. A worktree already there on that branch
/// is handed back as it is, so a host that asks again gets the same one.
pub(crate) async fn prepare(
    work: &Path,
    repo_url: Option<&str>,
    branch: &str,
    base: Option<&str>,
    user: Option<RunAs>,
) -> Result<Worktree, String> {
    check_name("branch", branch, false)?;
    if let Some(base) = base {
        check_name("base", base, true)?;
    }
    let repo = find_repo(work, repo_url)?;
    let path = path(work, branch);
    let git = Git {
        repo: &repo,
        user: user.as_ref(),
    };
    let done = || Worktree {
        path: path.display().to_string(),
        branch: branch.to_owned(),
    };

    if path.exists() {
        let head = git
            .run_in(&path, ["rev-parse", "--abbrev-ref", "HEAD"])
            .await?;
        if head.trim() == branch {
            return Ok(done());
        }
        return Err(format!(
            "{} is already there, on branch {}",
            path.display(),
            head.trim()
        ));
    }

    let local = format!("refs/heads/{branch}");
    let path_arg = path.as_os_str();
    if git.ok(["rev-parse", "--verify", "--quiet", &local]).await {
        // The branch outlived an earlier worktree, so it's checked out as
        // it is rather than made again.
        git.run([
            "worktree".as_ref(),
            "add".as_ref(),
            path_arg,
            branch.as_ref(),
        ])
        .await?;
        return Ok(done());
    }
    let remote = format!("refs/remotes/origin/{branch}");
    if base.is_none()
        && (git.ok(["rev-parse", "--verify", "--quiet", &remote]).await
            || git.fetched(branch).await)
    {
        // A branch already on the remote, such as a pull request's head,
        // is checked out tracking it, so the agent starts
        // from its commits and can push back to it.
        let upstream = format!("origin/{branch}");
        git.run([
            "worktree".as_ref(),
            "add".as_ref(),
            "--track".as_ref(),
            "-b".as_ref(),
            branch.as_ref(),
            path_arg,
            upstream.as_ref(),
        ])
        .await?;
        return Ok(done());
    }
    let base = match base {
        Some(base) => base.to_owned(),
        None if git
            .ok([
                "rev-parse",
                "--verify",
                "--quiet",
                "refs/remotes/origin/HEAD",
            ])
            .await =>
        {
            "origin/HEAD".to_owned()
        }
        None => "HEAD".to_owned(),
    };
    let args: [&OsStr; 6] = [
        "worktree".as_ref(),
        "add".as_ref(),
        "-b".as_ref(),
        branch.as_ref(),
        path_arg,
        base.as_ref(),
    ];
    git.run(args).await?;
    Ok(done())
}

/// Lists the files git ignores in the checkout at `cwd`, which must be
/// under the work folder, running git as `user`. A checkout that isn't
/// there lists nothing, as a listing git can't make does.
pub(crate) async fn ignored(work: &Path, cwd: &str, user: Option<RunAs>) -> Ignored {
    let Ok(checkout) = disk::inside(&[work.to_path_buf()], cwd) else {
        return Ignored::default();
    };
    let list = move || ignored_files::list(git(user.as_ref()), &checkout.path);
    tokio::task::spawn_blocking(list).await.unwrap_or_default()
}

/// Refuses a branch or base git would read as an option or a range, or
/// that has characters a branch name can't.
///
/// `branchProblem` in packages/shared/src/new-agent/branch.ts has the same
/// rules for a branch, so the new-agent form can say what's wrong before
/// Start. Both test the names in fixtures/branch-names.json, so a change
/// here goes there too.
fn check_name(what: &str, name: &str, base: bool) -> Result<(), String> {
    let fine = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-');
    let fine_base = |c: char| fine(c) || matches!(c, '~' | '^');
    let bad = name.is_empty()
        || name.starts_with('-')
        || name.starts_with('/')
        || name.ends_with('/')
        || name.ends_with('.')
        || name.ends_with(".lock")
        || name.contains("..")
        || name.contains("//")
        || name.contains("@{")
        || !name
            .chars()
            .all(|c| if base { fine_base(c) } else { fine(c) });
    if bad {
        return Err(format!("{name:?} can't be a {what}"));
    }
    Ok(())
}

/// The repo's folder: the one the repo URL names, or the only one there.
fn find_repo(work: &Path, repo_url: Option<&str>) -> Result<PathBuf, String> {
    if let Some(name) = repo_url.and_then(repo_folder) {
        let repo = work.join(name);
        if repo.join(".git").exists() {
            return Ok(repo);
        }
    }
    let entries = std::fs::read_dir(work)
        .map_err(|err| format!("couldn't read {}: {err}", work.display()))?;
    let mut repos: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join(".git").exists())
        .collect();
    match repos.len() {
        1 => Ok(repos.remove(0)),
        0 => Err(format!("there's no repo in {}", work.display())),
        _ => Err(format!(
            "{} has more than one repo, and the workspace's URL names none of them",
            work.display()
        )),
    }
}

/// Links a checkout under `work` to the workspace's skills mount: its
/// `.maxhq/skills` to the mount's set in use, a relative link per name in
/// each CLI's skills folder, and the exclude lines that keep them out of
/// git. Each of `homes`, a global skills folder under `home`, gets a link
/// per name in `always`. What's made goes to `user`, since the guest runs
/// as root. A pack lands from the mount and installs as `user`, and gives
/// where it stands.
pub(crate) fn link_skills(
    work: &Path,
    home: &Path,
    links: &SkillLinks<'_>,
    user: Option<&RunAs>,
) -> Result<Option<PackState>, String> {
    let root = Path::new(links.checkout);
    if !inside(root, work) || !really_in(root, &[work]) || !root.is_dir() {
        return Err(format!(
            "{} isn't a checkout in the work folder",
            root.display()
        ));
    }
    if let Some(folder) = links
        .homes
        .iter()
        .map(Path::new)
        .find(|folder| !inside(folder, home) || !really_in(folder, &[home, work]))
    {
        return Err(format!("{} isn't in the agent's home", folder.display()));
    }
    for name in links.names.iter().chain(links.always) {
        check_skill(name).map_err(|problem| format!("{name}: {problem}"))?;
    }
    let lock = SkillLock {
        skills: links
            .names
            .iter()
            .map(|name| LockedSkill {
                name: name.clone(),
                source: SkillSourceId::derive(name),
                path: String::new(),
                commit: None,
                tree: TreeHash(String::new()),
                pinned: false,
            })
            .collect(),
        committed: false,
        pack: None,
    };
    skill_lock::check(&lock, links.always).map_err(|err| err.to_string())?;
    let owner = user.map(|user| (user.uid, user.gid));
    let failed = |err: std::io::Error| format!("couldn't link the skills: {err}");
    // The links don't go through the git folder, so a checkout that keeps
    // it elsewhere still gets them and only goes without the exclude lines.
    let git_folder = match owner {
        Some(_) => apply::git_folder_in(work, root).map(|_| ()),
        None => Ok(()),
    };
    if let Err(error) = &git_folder {
        tracing::warn!(%error, checkout = %root.display(), "leaving the checkout's exclude file alone");
    }
    let plan =
        plan::checkout(&DiskFs, root, &lock, Layout::Mount, &BTreeMap::new()).map_err(failed)?;
    // The guest's answer has no room for a clash, so this tells the host.
    for clash in &plan.clashes {
        if let Clash::Outside(link) = clash {
            return Err(format!(
                "{} leads out of the checkout, so the skills weren't linked",
                link.display()
            ));
        }
    }
    for step in &plan.steps {
        apply::run(step, owner).map_err(failed)?;
    }
    apply::run(&Step::MakeDir(root.join(PROPOSALS_DIR)), owner).map_err(failed)?;
    let mut wanted = plan.exclude;
    if links.pack.is_some() {
        wanted.extend(pack::exclude_lines(root));
    }
    if git_folder.is_ok() {
        match owner {
            Some(owner) => apply::exclude_as(work, root, &wanted, owner),
            None => apply::exclude(root, &wanted, None),
        }
        .map_err(failed)?;
    }

    let wanted: BTreeMap<String, String> = links
        .always
        .iter()
        .map(|name| (name.clone(), format!("{MOUNT}/{name}")))
        .collect();
    let ours = |to: &Path| to.starts_with(crate::skills::sets::GUEST_PATH);
    for folder in links.homes {
        let (steps, _) = plan::links(&DiskFs, Path::new(folder), &wanted, &ours).map_err(failed)?;
        for step in &steps {
            apply::run(step, owner).map_err(failed)?;
        }
    }
    Ok(match links.pack {
        Some(tree) => {
            let landing = pack::Landing {
                root,
                from: &links.mount.join(PACK_DIR),
                tree,
                logs: links.logs,
                user,
                retry: links.retry,
                timeout: pack::TIMEOUT,
            };
            Some(pack::land(&landing, || {}))
        }
        None => pack::remove(root, links.logs, user.map(|user| (work, user))).map_err(failed)?,
    })
}

/// [`SkillFs`] inside an open folder. The agent can change a checkout while
/// the guest reads it as root, so each call opens from the folder and refuses
/// a link at any part.
struct BeneathFs(Dir);

impl BeneathFs {
    /// The folder `path` is in and its last part.
    fn parent(&self, path: &Path) -> io::Result<(Dir, String)> {
        let mut parts = path_parts(path)?;
        let name = parts.pop().ok_or(io::ErrorKind::InvalidInput)?;
        Ok((self.0.dir(&parts)?, name))
    }
}

/// `path`'s parts as [`Dir`] takes them.
fn path_parts(path: &Path) -> io::Result<Vec<String>> {
    path.components()
        .map(|part| match part.as_os_str().to_str() {
            Some(part) => Ok(part.to_owned()),
            None => Err(io::ErrorKind::InvalidInput.into()),
        })
        .collect()
}

impl SkillFs for BeneathFs {
    fn node(&self, path: &Path) -> io::Result<Option<Node>> {
        let (dir, name) = self.parent(path)?;
        Ok(match dir.kind(&name)? {
            None => None,
            Some(FileType::Symlink) => Some(Node::Link {
                target: String::new(),
            }),
            Some(FileType::Directory) => Some(Node::Dir),
            Some(FileType::RegularFile) => {
                let meta = dir.read_file(&name)?.metadata()?;
                Some(Node::File {
                    size: meta.len(),
                    executable: meta.permissions().mode() & 0o111 != 0,
                })
            }
            Some(_) => return Err(io::ErrorKind::InvalidInput.into()),
        })
    }

    fn real(&self, _path: &Path) -> io::Result<Option<PathBuf>> {
        Err(io::ErrorKind::Unsupported.into())
    }

    fn list(&self, path: &Path) -> io::Result<Vec<String>> {
        self.0.dir(&path_parts(path)?)?.names()
    }

    fn read(&self, path: &Path, limit: u64) -> io::Result<Vec<u8>> {
        let (dir, name) = self.parent(path)?;
        let file = dir.read_file(&name)?;
        if !file.metadata()?.is_file() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let mut bytes = Vec::new();
        file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

/// Reads the proposal folders of the checkout at `checkout` under
/// `work`, after removing the one called `remove`. Folders past
/// [`MAX_PROPOSAL_BYTES`] in all, and any with links, are left out.
pub(crate) fn read_proposals(
    work: &Path,
    checkout: &str,
    remove: Option<&str>,
) -> Result<Vec<GuestProposal>, String> {
    let root = Path::new(checkout);
    let real = |path: &Path| std::fs::canonicalize(path).ok();
    let within = match (real(root), real(work)) {
        (Some(root), Some(work)) => inside(&root, &work),
        _ => false,
    };
    if !inside(root, work) || !within || !root.is_dir() {
        return Err(format!("{checkout} isn't a checkout in the work folder"));
    }
    // A link here could reach past the checkout. This says so plainly, and the
    // open below refuses one that turns up after this look.
    let linked = |path: &Path| {
        std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
    };
    if linked(&root.join(".maxhq")) || linked(&root.join(PROPOSALS_DIR)) {
        return Err("The checkout's proposals folder is a link.".into());
    }
    if let Some(name) = remove {
        let plain = !name.is_empty() && !name.starts_with('.') && !name.contains('/');
        if !plain {
            return Err(format!("{name} isn't a proposal folder's name"));
        }
    }
    let parts = root
        .strip_prefix(work)
        .map_err(io::Error::other)
        .and_then(|checkout| path_parts(&checkout.join(PROPOSALS_DIR)))
        .map_err(|err| format!("{checkout} isn't a checkout in the work folder: {err}"))?;
    let dir = match Dir::open(work).and_then(|work| work.dir(&parts)) {
        Ok(dir) => dir,
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(Vec::new())
        }
        Err(err) => return Err(format!("couldn't open the proposals folder: {err}")),
    };
    if let Some(name) = remove {
        dir.remove_tree(name)
            .map_err(|err| format!("couldn't remove the proposal: {err}"))?;
    }
    let mut names = dir.names().unwrap_or_default();
    names.retain(|name| !name.starts_with('.'));
    names.sort();
    let mut found = Vec::new();
    let mut bytes = 0;
    for name in names {
        let Ok(folder) = dir.dir(std::slice::from_ref(&name)) else {
            continue;
        };
        let folder = BeneathFs(folder);
        let Ok(files) = tree::read(&folder, Path::new("")) else {
            continue;
        };
        let size: u64 = files.iter().map(|entry| entry.size).sum();
        if bytes + size > MAX_PROPOSAL_BYTES {
            continue;
        }
        let mut read = Vec::new();
        for entry in files.iter().filter(|entry| entry.kind == EntryKind::File) {
            let mut data = folder
                .read(Path::new(&entry.path), entry.size)
                .map_err(|err| format!("couldn't read {}: {err}", entry.path))?;
            data.truncate(usize::try_from(entry.size).unwrap_or(usize::MAX));
            read.push(GuestProposalFile {
                path: entry.path.clone(),
                executable: entry.executable,
                data: STANDARD.encode(data),
            });
        }
        bytes += size;
        found.push(GuestProposal {
            folder: name,
            files: read,
        });
    }
    Ok(found)
}

/// Whether `path` is an absolute path in `base`, with no `..` to leave it.
fn inside(path: &Path, base: &Path) -> bool {
    path.is_absolute()
        && path.starts_with(base)
        && !path
            .components()
            .any(|part| part == std::path::Component::ParentDir)
}

/// Whether `path` stands in one of `bases` with links followed, as far as
/// it exists yet. Root does what's asked here, and the agent's user can
/// link any name it owns to anywhere.
fn really_in(path: &Path, bases: &[&Path]) -> bool {
    let Some(real) = path
        .ancestors()
        .find(|at| at.symlink_metadata().is_ok())
        .and_then(|at| std::fs::canonicalize(at).ok())
    else {
        return false;
    };
    // A base that isn't there yet can't lead anywhere else.
    bases.iter().any(|base| match std::fs::canonicalize(base) {
        Ok(base) => real.starts_with(base),
        Err(_) => path.starts_with(base),
    })
}

/// What a [`link_skills`] links, as the host asked.
pub(crate) struct SkillLinks<'a> {
    /// The checkout's absolute path.
    pub(crate) checkout: &'a str,
    /// The project's skills.
    pub(crate) names: &'a [String],
    /// The skills every agent gets.
    pub(crate) always: &'a [String],
    /// The global skills folders the always set goes in.
    pub(crate) homes: &'a [String],
    /// The devflow pack's tree, none to take MaxHQ's copy out.
    pub(crate) pack: Option<&'a TreeHash>,
    /// Whether a failed install runs again.
    pub(crate) retry: bool,
    /// Where the skills mount is, which holds the pack.
    pub(crate) mount: &'a Path,
    /// Where the installer's logs go.
    pub(crate) logs: &'a Path,
}

/// The folder `git clone` makes for a URL: its last part, without `.git`.
fn repo_folder(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let last = url.rsplit(['/', ':']).next()?;
    let last = last.strip_suffix(".git").unwrap_or(last);
    let fine = !last.is_empty() && last != "." && last != "..";
    fine.then(|| last.to_owned())
}

/// Git as `user`, or as this process, which never asks for a password.
pub(crate) fn git(user: Option<&RunAs>) -> std::process::Command {
    use std::os::unix::process::CommandExt;

    let mut command = std::process::Command::new("git");
    command.env("GIT_TERMINAL_PROMPT", "0");
    // Tests run git as whoever runs them, so their own config and
    // hooks stay out of it.
    // A git hook sets these, and they would point every call at the
    // repo running the hook and commit as whoever runs it.
    #[cfg(test)]
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_AUTHOR_NAME")
        .env_remove("GIT_AUTHOR_EMAIL")
        .env_remove("GIT_AUTHOR_DATE")
        .env_remove("GIT_COMMITTER_NAME")
        .env_remove("GIT_COMMITTER_EMAIL")
        .env_remove("GIT_COMMITTER_DATE");
    if let Some(user) = user {
        command
            .env_clear()
            .env("PATH", PATH)
            .env("HOME", &user.home)
            .env("USER", &user.user)
            .env("LOGNAME", &user.user)
            .env("GIT_TERMINAL_PROMPT", "0")
            .uid(user.uid)
            .gid(user.gid);
    }
    command
}

/// Runs git in the repo as the worktree's user.
struct Git<'a> {
    repo: &'a Path,
    user: Option<&'a RunAs>,
}

impl Git<'_> {
    fn command(&self, dir: &Path) -> Command {
        let mut command = Command::from(git(self.user));
        command
            .arg("-C")
            .arg(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// Whether git succeeds.
    async fn ok<I, S>(&self, args: I) -> bool
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.command(self.repo)
            .args(args)
            .status()
            .await
            .is_ok_and(|status| status.success())
    }

    /// Whether `branch` could be fetched from origin into its
    /// remote-tracking ref. It gives up after [`FETCH_TIMEOUT`], so a
    /// container without a network still gets its worktree.
    async fn fetched(&self, branch: &str) -> bool {
        let refspec = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");
        let mut command = self.command(self.repo);
        command
            .args(["fetch", "--quiet", "--no-tags", "origin", &refspec])
            .kill_on_drop(true);
        matches!(
            tokio::time::timeout(FETCH_TIMEOUT, command.status()).await,
            Ok(Ok(status)) if status.success()
        )
    }

    /// Runs git in the repo, giving back what it printed.
    async fn run<I, S>(&self, args: I) -> Result<String, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_in(self.repo, args).await
    }

    /// Runs git in `dir`, giving back what it printed.
    async fn run_in<I, S>(&self, dir: &Path, args: I) -> Result<String, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self
            .command(dir)
            .args(args)
            .output()
            .await
            .map_err(|err| format!("couldn't run git: {err}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("git failed: {}", stderr.trim()));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{symlink, MetadataExt};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::testing::TempDir;

    /// How many times the tests that race an agent read the checkout.
    const RACE_READS: usize = 2000;

    /// A work folder and a checkout in it with the proposals folder made.
    fn checkout(temp: &TempDir) -> (PathBuf, PathBuf, PathBuf) {
        let work = temp.path().join("work");
        let checkout = work.join("repo");
        let proposals = checkout.join(PROPOSALS_DIR);
        std::fs::create_dir_all(&proposals).unwrap();
        (work, checkout, proposals)
    }

    /// Runs `change` over and over on a thread while `body` runs.
    fn racing(change: impl Fn() + Send + 'static, body: impl FnOnce()) {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                change();
            }
        });
        body();
        stop.store(true, Ordering::Relaxed);
        thread.join().unwrap();
    }

    #[test]
    fn proposals_read_with_their_modes_and_go_by_name() {
        let temp = TempDir::new();
        let (work, checkout, proposals) = checkout(&temp);
        let scripts = proposals.join("one").join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(proposals.join("one").join("SKILL.md"), "one").unwrap();
        std::fs::write(scripts.join("run.sh"), "run").unwrap();
        std::fs::set_permissions(
            scripts.join("run.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::fs::create_dir_all(proposals.join("two")).unwrap();
        std::fs::write(proposals.join("two").join("SKILL.md"), "two").unwrap();
        std::fs::write(proposals.join("a-file"), "not a folder").unwrap();
        let checkout = checkout.to_str().unwrap();

        let found = read_proposals(&work, checkout, None).unwrap();
        let folders: Vec<_> = found.iter().map(|one| one.folder.as_str()).collect();
        assert_eq!(folders, ["one", "two"]);
        let files: Vec<_> = found[0]
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.executable, file.data.as_str()))
            .collect();
        assert_eq!(
            files,
            [
                ("SKILL.md", false, STANDARD.encode("one").as_str()),
                ("scripts/run.sh", true, STANDARD.encode("run").as_str()),
            ]
        );

        let found = read_proposals(&work, checkout, Some("one")).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].folder, "two");
        assert!(!proposals.join("one").exists());
    }

    #[test]
    fn a_proposal_with_a_link_or_a_pipe_is_left_out() {
        let temp = TempDir::new();
        let (work, checkout, proposals) = checkout(&temp);
        let secret = temp.path().join("secret");
        std::fs::write(&secret, "secret").unwrap();
        std::fs::create_dir_all(proposals.join("linked")).unwrap();
        symlink(&secret, proposals.join("linked").join("file")).unwrap();
        symlink(temp.path(), proposals.join("folder-link")).unwrap();
        std::fs::create_dir_all(proposals.join("piped")).unwrap();
        let made = std::process::Command::new("mkfifo")
            .arg(proposals.join("piped").join("pipe"))
            .status()
            .unwrap();
        assert!(made.success());
        std::fs::create_dir_all(proposals.join("plain")).unwrap();
        std::fs::write(proposals.join("plain").join("file"), "plain").unwrap();

        let found = read_proposals(&work, checkout.to_str().unwrap(), None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].folder, "plain");
    }

    #[test]
    fn a_file_swapped_for_a_link_never_gives_what_it_points_at() {
        let temp = TempDir::new();
        let (work, checkout, proposals) = checkout(&temp);
        let secret = temp.path().join("secret");
        std::fs::write(&secret, "root's secret").unwrap();
        let folder = proposals.join("p");
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("file");
        std::fs::write(&file, "plain").unwrap();
        let (plain, link) = (temp.path().join("plain"), temp.path().join("link"));
        let (from, to) = (work.clone(), checkout.to_str().unwrap().to_owned());

        racing(
            move || {
                std::fs::write(&plain, "plain").unwrap();
                std::fs::rename(&plain, &file).unwrap();
                symlink(&secret, &link).unwrap();
                std::fs::rename(&link, &file).unwrap();
            },
            || {
                for _ in 0..RACE_READS {
                    let Ok(found) = read_proposals(&from, &to, None) else {
                        continue;
                    };
                    for file in found.iter().flat_map(|one| &one.files) {
                        assert_eq!(file.data, STANDARD.encode("plain"));
                    }
                }
            },
        );
    }

    #[test]
    fn a_folder_swapped_for_a_link_never_loses_what_it_points_at() {
        let temp = TempDir::new();
        let (work, checkout, proposals) = checkout(&temp);
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(outside.join("x")).unwrap();
        std::fs::write(outside.join("x").join("kept"), "kept").unwrap();
        let (aside, link) = (temp.path().join("aside"), temp.path().join("link"));
        let (from, to) = (work.clone(), checkout.to_str().unwrap().to_owned());
        let target = outside.clone();

        racing(
            move || {
                std::fs::rename(&proposals, &aside).unwrap();
                symlink(&target, &link).unwrap();
                std::fs::rename(&link, &proposals).unwrap();
                std::fs::remove_file(&proposals).unwrap();
                std::fs::rename(&aside, &proposals).unwrap();
            },
            || {
                for _ in 0..RACE_READS {
                    let _ = read_proposals(&from, &to, Some("x"));
                    assert!(outside.join("x").join("kept").exists());
                }
            },
        );
    }

    /// A work folder, a home and a folder outside both, which nothing here
    /// may touch.
    struct Linking {
        _dir: TempDir,
        work: PathBuf,
        home: PathBuf,
        outside: PathBuf,
    }

    fn linking() -> Linking {
        let dir = TempDir::new();
        let [work, home, outside] = ["work", "home", "outside"].map(|name| dir.path().join(name));
        for folder in [&work, &home, &outside] {
            std::fs::create_dir(folder).unwrap();
        }
        Linking {
            _dir: dir,
            work,
            home,
            outside,
        }
    }

    impl Linking {
        /// Links the checkout at `checkout` as root would for this user,
        /// which is the only user a test can hand things to.
        fn link(&self, checkout: &Path, homes: &[String]) -> Result<(), String> {
            let mine = std::fs::metadata(&self.work).unwrap();
            let user = RunAs {
                user: "dev".to_owned(),
                uid: mine.uid(),
                gid: mine.gid(),
                home: self.home.clone(),
            };
            let checkout = checkout.display().to_string();
            let always = if homes.is_empty() {
                Vec::new()
            } else {
                vec!["git-flow".to_owned()]
            };
            let links = SkillLinks {
                checkout: &checkout,
                names: &[],
                always: &always,
                homes,
                pack: None,
                retry: false,
                mount: &self.outside,
                logs: &self.outside,
            };
            link_skills(&self.work, &self.home, &links, Some(&user)).map(|_| ())
        }

        fn outside_is_empty(&self) {
            let left: Vec<_> = std::fs::read_dir(&self.outside)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            assert!(left.is_empty(), "{left:?}");
        }

        /// A checkout in the work folder with a `.git` folder.
        fn repo(&self, name: &str) -> PathBuf {
            let root = self.work.join(name);
            std::fs::create_dir_all(root.join(".git")).unwrap();
            root
        }
    }

    #[test]
    fn a_checkout_linked_out_of_the_work_folder_is_refused() {
        let s = linking();
        let root = s.work.join("project");
        symlink(&s.outside, &root).unwrap();
        let refused = s.link(&root, &[]).unwrap_err();
        assert!(refused.contains("isn't a checkout"), "{refused}");
        s.outside_is_empty();
    }

    #[test]
    fn a_home_folder_linked_out_of_the_home_is_refused() {
        let s = linking();
        let root = s.repo("project");
        symlink(&s.outside, s.home.join(".agents")).unwrap();
        let folder = s.home.join(".agents/skills").display().to_string();
        let refused = s.link(&root, &[folder]).unwrap_err();
        assert!(refused.contains("isn't in the agent's home"), "{refused}");
        s.outside_is_empty();

        // A link that stays in the home or the work folder is the owner's
        // own setup.
        std::fs::remove_file(s.home.join(".agents")).unwrap();
        std::fs::create_dir(s.home.join("dotfiles")).unwrap();
        symlink(s.home.join("dotfiles"), s.home.join(".agents")).unwrap();
        let folder = s.home.join(".agents/skills").display().to_string();
        s.link(&root, &[folder]).unwrap();
        assert!(s
            .home
            .join("dotfiles/skills/git-flow")
            .symlink_metadata()
            .is_ok());
    }

    #[test]
    fn a_git_folder_named_outside_the_work_folder_gets_links_and_no_exclude_lines() {
        let s = linking();
        let root = s.work.join("project");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join(".git"),
            format!("gitdir: {}\n", s.outside.display()),
        )
        .unwrap();
        s.link(&root, &[]).unwrap();
        s.outside_is_empty();
        assert!(root.join(".maxhq/proposals").is_dir());
    }

    #[test]
    fn a_commondir_outside_the_work_folder_gets_no_exclude_lines() {
        let s = linking();
        let root = s.work.join("project");
        let own = s.work.join("own");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(own.join("commondir"), format!("{}\n", s.outside.display())).unwrap();
        std::fs::write(root.join(".git"), format!("gitdir: {}\n", own.display())).unwrap();
        s.link(&root, &[]).unwrap();
        s.outside_is_empty();
    }

    #[test]
    fn a_linked_info_folder_gets_nothing() {
        let s = linking();
        let root = s.repo("project");
        symlink(&s.outside, root.join(".git/info")).unwrap();
        assert!(s.link(&root, &[]).is_err());
        s.outside_is_empty();
    }

    #[test]
    fn a_linked_exclude_file_is_never_read_back_into_the_repo() {
        let s = linking();
        let root = s.repo("project");
        let secret = s.outside.join("secret");
        std::fs::write(&secret, "not the guest's").unwrap();
        std::fs::create_dir(root.join(".git/info")).unwrap();
        let exclude = root.join(".git/info/exclude");
        symlink(&secret, &exclude).unwrap();

        assert!(s.link(&root, &[]).is_err());
        assert!(std::fs::symlink_metadata(&exclude).unwrap().is_symlink());
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "not the guest's");
    }

    #[test]
    fn a_second_name_for_someone_elses_exclude_file_is_never_read() {
        let s = linking();
        let root = s.repo("project");
        let secret = s.outside.join("secret");
        std::fs::write(&secret, "not the guest's").unwrap();
        std::fs::create_dir(root.join(".git/info")).unwrap();
        std::fs::hard_link(&secret, root.join(".git/info/exclude")).unwrap();

        assert!(s.link(&root, &[]).is_err());
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "not the guest's");
    }

    #[test]
    fn a_linked_worktree_in_the_work_folder_still_gets_its_exclude_lines() {
        let s = linking();
        let main = s.repo("main");
        std::fs::write(main.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir(main.join(".git/objects")).unwrap();
        std::fs::create_dir(main.join(".git/refs")).unwrap();
        let own = main.join(".git/worktrees/branch");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("commondir"), "../..\n").unwrap();
        let linked = s.work.join("branch");
        std::fs::create_dir(&linked).unwrap();
        std::fs::write(linked.join(".git"), format!("gitdir: {}\n", own.display())).unwrap();

        s.link(&linked, &[]).unwrap();

        let info = main.join(".git/info");
        assert_eq!(
            std::fs::read_to_string(info.join("exclude")).unwrap(),
            "# MaxHQ skills\n/.maxhq/\n"
        );
        assert_eq!(std::fs::read_dir(&info).unwrap().count(), 1);
        assert!(!own.join("info").exists());

        // A plain repo, an existing file, and a second run that changes
        // nothing.
        let plain = s.repo("plain");
        std::fs::create_dir(plain.join(".git/info")).unwrap();
        std::fs::write(plain.join(".git/info/exclude"), "*.log\n").unwrap();
        s.link(&plain, &[]).unwrap();
        s.link(&plain, &[]).unwrap();
        assert_eq!(
            std::fs::read_to_string(plain.join(".git/info/exclude")).unwrap(),
            "*.log\n# MaxHQ skills\n/.maxhq/\n"
        );
    }

    #[test]
    fn names_git_would_misread_are_refused() {
        for name in [
            "", "-b", "a..b", "a@{1}", "a b", "a/", "/a", "a.lock", "a//b", "a;b",
        ] {
            assert!(check_name("branch", name, false).is_err(), "{name:?}");
        }
        for name in ["agent/fix-login", "v1.2", "feature_x"] {
            assert!(check_name("branch", name, false).is_ok(), "{name:?}");
        }
        assert!(check_name("branch", "main~1", false).is_err());
        assert!(check_name("base", "main~1", true).is_ok());
        assert!(check_name("base", "origin/main^", true).is_ok());
    }

    #[test]
    fn the_form_checks_branches_the_same_way() {
        // The names the new-agent form's check is tested on too.
        #[derive(serde::Deserialize)]
        struct Names {
            good: Vec<String>,
            bad: Vec<String>,
        }
        let names: Names =
            serde_json::from_str(include_str!("../../fixtures/branch-names.json")).unwrap();
        for name in &names.good {
            assert!(check_name("branch", name, false).is_ok(), "{name:?}");
        }
        for name in &names.bad {
            assert!(check_name("branch", name, false).is_err(), "{name:?}");
        }
    }

    #[test]
    fn the_folder_comes_from_the_url() {
        for (url, folder) in [
            ("https://github.com/example-user/maxhq.git", "maxhq"),
            ("git@github.com:example-user/maxhq.git", "maxhq"),
            ("https://github.com/example-user/maxhq/", "maxhq"),
            ("example-user/maxhq", "maxhq"),
        ] {
            assert_eq!(repo_folder(url).as_deref(), Some(folder), "{url}");
        }
        assert_eq!(repo_folder("https://github.com/o/.."), None);
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    #[tokio::test]
    async fn a_worktree_lands_on_its_branch_once() {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let temp = TempDir::new();
        let work = temp.path().join("work");
        let repo = work.join("demo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "first",
            ],
        );

        let url = Some("https://github.com/example-user/demo.git");
        let made = prepare(&work, url, "agent/one", None, None).await.unwrap();
        assert_eq!(made.branch, "agent/one");
        assert_eq!(
            made.path,
            work.join(".worktrees/agent-one").display().to_string()
        );
        assert!(Path::new(&made.path).join(".git").exists());

        let again = prepare(&work, url, "agent/one", None, None).await.unwrap();
        assert_eq!(again, made);

        let based = prepare(&work, None, "agent/two", Some("main"), None)
            .await
            .unwrap();
        assert_eq!(
            based.path,
            work.join(".worktrees/agent-two").display().to_string()
        );

        let err = prepare(&work, url, "-x", None, None).await.unwrap_err();
        assert!(err.contains("can't be a branch"), "{err}");
        let err = prepare(&work, url, "agent/three", Some("nowhere"), None)
            .await
            .unwrap_err();
        assert!(err.starts_with("git failed"), "{err}");
    }

    fn commit(dir: &Path, message: &str) {
        git(
            dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                message,
            ],
        );
    }

    fn read(dir: &str, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[tokio::test]
    async fn a_branch_on_the_remote_is_tracked() {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let temp = TempDir::new();
        let origin = temp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        commit(&origin, "first");
        git(&origin, &["checkout", "-q", "-b", "pr/one"]);
        commit(&origin, "one");
        git(&origin, &["checkout", "-q", "main"]);
        let work = temp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["clone", "-q", origin.to_str().unwrap(), "demo"]);
        // Pushed after the clone, so only a fetch finds it.
        git(&origin, &["checkout", "-q", "-b", "pr/two"]);
        commit(&origin, "two");
        git(&origin, &["checkout", "-q", "main"]);

        let origin_dir = origin.to_str().unwrap();
        for name in ["pr/one", "pr/two"] {
            let made = prepare(&work, None, name, None, None).await.unwrap();
            assert_eq!(
                read(&made.path, &["rev-parse", "HEAD"]),
                read(origin_dir, &["rev-parse", name]),
                "{name}"
            );
            assert_eq!(
                read(&made.path, &["rev-parse", "--abbrev-ref", "@{u}"]),
                format!("origin/{name}")
            );
        }

        let fresh = prepare(&work, None, "agent/new", None, None).await.unwrap();
        assert_eq!(
            read(&fresh.path, &["rev-parse", "HEAD"]),
            read(origin_dir, &["rev-parse", "main"])
        );
        assert_eq!(
            read(&fresh.path, &["rev-parse", "--abbrev-ref", "@{u}"]),
            ""
        );
    }
}
