// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Where a library's skills come from. A git source is a clone the
//! holder keeps, brought up to its remote with the git CLI, never with a
//! prompt for a password. A local source is a folder on the holder, read
//! where it is. Reading a source lands each skill in the store, so its
//! [`SkillInfo`] pins it to a tree.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use maxhq_core::skills::disk::DiskFs;
use maxhq_core::skills::frontmatter::{self, SkillDoc};
use maxhq_core::skills::{ignored, pack, Node, SkillFs, SKILL_FILE};
use maxhq_protocol::ids::SkillSourceId;
use maxhq_protocol::skills::{SkillInfo, TreeHash};

use super::store::SkillStore;
use crate::guest::worktree::git;

/// How deep below a source's root a skill folder can be, as in
/// `skills/<group>/<name>/`.
const MAX_DEPTH: usize = 4;

/// The kinds of remote a source's clone may use. A URL like `evil::x` would
/// run `git-remote-evil` from the PATH. A folder source is read where it is
/// and never reaches git, so `file` is here for tests alone.
const ALLOWED_PROTOCOLS: &str = if cfg!(test) {
    "http:https:ssh:git:file"
} else {
    "http:https:ssh:git"
};

/// Clones `url` into `dir`, at `branch` when one is named.
pub fn clone(url: &str, branch: Option<&str>, dir: &Path) -> Result<(), String> {
    run(clone_command(url, branch, dir)).map(drop)
}

fn clone_command(url: &str, branch: Option<&str>, dir: &Path) -> Command {
    let mut command = git(None);
    command
        .env("GIT_ALLOW_PROTOCOL", ALLOWED_PROTOCOLS)
        .args(["clone", "--quiet"]);
    if let Some(branch) = branch {
        command.args(["--branch", branch]);
    }
    command.arg("--").arg(url).arg(dir);
    command
}

/// Fetches the remote's `branch`, or its default branch, into the clone at
/// `dir`. The branch comes from a source's settings, so `--` keeps it from
/// reading as an option.
fn fetch_command(dir: &Path, branch: Option<&str>) -> Command {
    let mut command = git(None);
    command
        .env("GIT_ALLOW_PROTOCOL", ALLOWED_PROTOCOLS)
        .arg("-C")
        .arg(dir)
        .args(["fetch", "--quiet", "--", "origin", branch.unwrap_or("HEAD")]);
    command
}

/// Brings the clone at `dir` up to its remote's `branch`, or the remote's
/// default branch. A clone with commits the remote doesn't have is left
/// alone, since they're changes nobody pushed.
pub fn update(dir: &Path, branch: Option<&str>) -> Result<(), String> {
    run(fetch_command(dir, branch))?;
    let ahead = in_repo(dir, &["rev-list", "--count", "FETCH_HEAD..HEAD"])?;
    if ahead.trim() != "0" {
        return Err("The clone has commits the remote doesn't, so it wasn't updated.".into());
    }
    in_repo(dir, &["reset", "--quiet", "--hard", "FETCH_HEAD"]).map(drop)
}

/// The commit the clone at `dir` is at.
pub fn head(dir: &Path) -> Result<String, String> {
    in_repo(dir, &["rev-parse", "HEAD"]).map(|out| out.trim().to_owned())
}

/// Whether the clone at `dir` has commits no branch of its remote has.
pub fn unpushed(dir: &Path) -> bool {
    in_repo(dir, &["rev-list", "--count", "HEAD", "--not", "--remotes"])
        .is_ok_and(|count| count.trim() != "0")
}

/// Pushes the repo at `dir` to its remote's branch of the same name.
pub fn push(dir: &Path) -> Result<(), String> {
    if !dir.join(".git").exists() {
        return Err("The source isn't a git repo, so there's nothing to push.".into());
    }
    if in_repo(dir, &["remote"])?.trim().is_empty() {
        return Err("The source has no remote to push to.".into());
    }
    in_repo(dir, &["push", "--quiet", "origin", "HEAD"]).map(drop)
}

/// Runs git in the repo at `dir` and gives what it printed.
pub(super) fn in_repo(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = git(None);
    command.arg("-C").arg(dir).args(args);
    run(command)
}

fn run(mut command: Command) -> Result<String, String> {
    command.env("LC_ALL", "C").stdin(Stdio::null());
    let out = command
        .output()
        .map_err(|err| format!("Couldn't run git: {err}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    let said = said.lines().rev().find(|line| !line.trim().is_empty());
    Err(match said {
        Some(line) => format!("git said: {}", line.trim()),
        None => format!("git ended with {}", out.status),
    })
}

/// A source's skills, each landed in `store`.
#[derive(Debug, Default)]
pub struct Read {
    /// Its skills, by path.
    pub skills: Vec<SkillInfo>,
    /// The skill folders it left out, and why.
    pub skipped: Vec<String>,
}

impl Read {
    /// What to tell the app about the skipped folders, if any.
    pub fn problem(&self) -> Option<String> {
        (!self.skipped.is_empty())
            .then(|| format!("Some skills were left out: {}", self.skipped.join("; ")))
    }
}

/// Reads every skill folder under `root`: a folder with a `SKILL.md`,
/// up to [`MAX_DEPTH`] deep. Dot folders and links are passed over.
pub fn read(root: &Path, source: SkillSourceId, store: &SkillStore) -> Read {
    let mut found = Read::default();
    for (path, folder) in skill_folders(root) {
        match skill(&path, &folder, source, store) {
            Ok(info) => found.skills.push(info),
            Err(why) => found.skipped.push(format!("{} {why}", shown(&path))),
        }
    }
    found.skills.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

/// The skill at `folder`, `path` inside its source, landed in `store`.
pub fn skill(
    path: &str,
    folder: &Path,
    source: SkillSourceId,
    store: &SkillStore,
) -> Result<SkillInfo, String> {
    let doc = doc(folder)?;
    let fallback = folder
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let name = doc.name().unwrap_or(fallback).to_owned();
    frontmatter::check_name(&name).map_err(|why| format!("has a name that {why}"))?;
    let (tree, held) = store.copy_in(folder)?;
    Ok(SkillInfo {
        description: doc.description().unwrap_or_default().to_owned(),
        flags: frontmatter::flags(&doc, &held.entries),
        listing_chars: doc.listing_chars(&name),
        name,
        tree,
        path: path.to_owned(),
        source,
    })
}

fn doc(folder: &Path) -> Result<SkillDoc, String> {
    let text = maxhq_core::fs::read_text(
        &folder.join(SKILL_FILE),
        maxhq_core::skills::tree::MAX_BYTES,
    )
    .map_err(|err| format!("couldn't be read: {err}"))?;
    frontmatter::parse(&text).map_err(|err| format!("has a {SKILL_FILE} that can't be read: {err}"))
}

fn shown(path: &str) -> &str {
    if path.is_empty() {
        "The source's root"
    } else {
        path
    }
}

/// The devflow pack of the repo at `dir`, when its root has the
/// installer: git's files at its commit, with their modes, built in
/// `scratch` and landed in `store`. Links, submodules and what git
/// ignores are left out.
pub fn pack(dir: &Path, scratch: &Path, store: &SkillStore) -> Result<Option<TreeHash>, String> {
    use std::os::unix::fs::PermissionsExt;

    let listed = in_repo(dir, &["ls-tree", "-r", "-z", "HEAD"])?;
    let files: Vec<(&str, u32)> = listed
        .split('\0')
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            match meta.split(' ').next()? {
                "100755" => Some((path, 0o755)),
                "100644" => Some((path, 0o644)),
                _ => None,
            }
        })
        .collect();
    if !files.iter().any(|(path, _)| *path == pack::INSTALLER) {
        return Ok(None);
    }
    let built = files.iter().try_for_each(|(path, mode)| {
        let to = scratch.join(path);
        fs::create_dir_all(to.parent().unwrap_or(scratch))?;
        fs::copy(dir.join(path), &to)?;
        fs::set_permissions(&to, fs::Permissions::from_mode(*mode))
    });
    let landed = built
        .map_err(|err| format!("couldn't copy the pack: {err}"))
        .and_then(|()| store.copy_in(scratch));
    let _ = fs::remove_dir_all(scratch);
    landed.map(|(tree, _)| Some(tree))
}

/// Each skill folder under `root`, as its path inside `root` and where it
/// is. A skill's own folders aren't searched for more.
pub fn skill_folders(root: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let mut todo = vec![(String::new(), 0)];
    while let Some((path, depth)) = todo.pop() {
        let dir = if path.is_empty() {
            root.to_path_buf()
        } else {
            root.join(&path)
        };
        if matches!(
            DiskFs.node(&dir.join(SKILL_FILE)),
            Ok(Some(Node::File { .. }))
        ) {
            found.push((path, dir));
            continue;
        }
        if depth == MAX_DEPTH {
            continue;
        }
        let Ok(names) = DiskFs.list(&dir) else {
            continue;
        };
        for name in names {
            if ignored(&name) || name.starts_with('.') || name == "node_modules" {
                continue;
            }
            if let Ok(Some(Node::Dir)) = DiskFs.node(&dir.join(&name)) {
                let inner = if path.is_empty() {
                    name
                } else {
                    format!("{path}/{name}")
                };
                todo.push((inner, depth + 1));
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
pub(super) mod tests {
    use maxhq_core::skills::store::TreeStore;

    use super::*;
    use crate::testing::TempDir;

    /// Git with a name to commit as, for test repos.
    pub fn commit_all(repo: &Path, message: &str) {
        in_repo(repo, &["add", "-A"]).unwrap();
        in_repo(
            repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--quiet",
                "-m",
                message,
            ],
        )
        .unwrap();
    }

    /// A repo at `dir` with the skills named, under `skills/`.
    pub fn repo(dir: &Path, skills: &[(&str, &str)]) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        in_repo(dir, &["init", "--quiet", "--initial-branch=main"]).unwrap();
        write_skills(dir, skills);
        commit_all(dir, "skills");
        dir.to_path_buf()
    }

    pub fn write_skills(dir: &Path, skills: &[(&str, &str)]) {
        for (name, description) in skills {
            let folder = dir.join("skills").join(name);
            fs::create_dir_all(&folder).unwrap();
            fs::write(
                folder.join(SKILL_FILE),
                format!("---\nname: {name}\ndescription: {description}\n---\nDo it.\n"),
            )
            .unwrap();
        }
    }

    #[test]
    fn a_pack_is_git_s_files_at_the_commit_with_their_modes() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new();
        let root = repo(&dir.path().join("pack"), &[("alpha", "First")]);
        let store = SkillStore::under(&dir.path().join("data"));
        let scratch = dir.path().join("scratch");
        assert_eq!(pack(&root, &scratch, &store).unwrap(), None);

        fs::write(root.join(".gitignore"), ".venv/\n").unwrap();
        fs::write(root.join(pack::INSTALLER), "exit 0\n").unwrap();
        let script = fs::Permissions::from_mode(0o755);
        fs::set_permissions(root.join(pack::INSTALLER), script).unwrap();
        commit_all(&root, "installer");
        fs::create_dir_all(root.join(".venv")).unwrap();
        fs::write(root.join(".venv/local"), "state").unwrap();
        fs::write(root.join("untracked"), "mine").unwrap();

        let tree = pack(&root, &scratch, &store).unwrap().unwrap();
        let landed = store.path(&tree).unwrap();
        let mode = fs::metadata(landed.join(pack::INSTALLER))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111);
        assert!(landed.join("skills/alpha").join(SKILL_FILE).is_file());
        for left_out in [".git", ".venv", "untracked"] {
            assert!(!landed.join(left_out).exists(), "{left_out}");
        }
        assert!(!scratch.exists());
        // The same files give the same tree.
        assert_eq!(pack(&root, &scratch, &store).unwrap(), Some(tree));
        store.open_up();
    }

    #[test]
    fn a_clone_reads_each_skill_and_pins_it_to_a_tree() {
        let dir = TempDir::new();
        let remote = repo(
            &dir.path().join("remote"),
            &[("alpha", "First"), ("beta", "Second")],
        );
        let store = SkillStore::under(&dir.path().join("data"));
        let clone_dir = dir.path().join("clone");
        clone(remote.to_str().unwrap(), None, &clone_dir).unwrap();
        assert_eq!(head(&clone_dir).unwrap(), head(&remote).unwrap());

        let source = SkillSourceId::derive("test");
        let found = read(&clone_dir, source, &store);
        assert!(found.skipped.is_empty(), "{:?}", found.skipped);
        let names: Vec<_> = found
            .skills
            .iter()
            .map(|s| (s.name.as_str(), s.path.as_str()))
            .collect();
        assert_eq!(names, [("alpha", "skills/alpha"), ("beta", "skills/beta")]);
        let alpha = &found.skills[0];
        assert_eq!(alpha.description, "First");
        assert_eq!(alpha.source, source);
        assert!(store.has(&alpha.tree));
        store.open_up();
    }

    #[test]
    fn an_update_moves_to_the_remote_and_changes_the_tree() {
        let dir = TempDir::new();
        let remote = repo(&dir.path().join("remote"), &[("alpha", "First")]);
        let store = SkillStore::under(&dir.path().join("data"));
        let clone_dir = dir.path().join("clone");
        clone(remote.to_str().unwrap(), Some("main"), &clone_dir).unwrap();
        let source = SkillSourceId::derive("test");
        let before = read(&clone_dir, source, &store).skills[0].tree.clone();

        write_skills(&remote, &[("alpha", "Changed")]);
        commit_all(&remote, "change");
        update(&clone_dir, Some("main")).unwrap();
        assert_eq!(head(&clone_dir).unwrap(), head(&remote).unwrap());
        let after = read(&clone_dir, source, &store).skills[0].clone();
        assert_eq!(after.description, "Changed");
        assert_ne!(after.tree, before);
        // The older tree stays for the projects that locked it.
        assert!(store.has(&before));
        store.open_up();
    }

    #[test]
    fn a_clone_with_its_own_commits_isnt_updated() {
        let dir = TempDir::new();
        let remote = repo(&dir.path().join("remote"), &[("alpha", "First")]);
        let clone_dir = dir.path().join("clone");
        clone(remote.to_str().unwrap(), None, &clone_dir).unwrap();
        write_skills(&clone_dir, &[("local", "Mine")]);
        commit_all(&clone_dir, "local");
        let mine = head(&clone_dir).unwrap();

        let refused = update(&clone_dir, None).unwrap_err();
        assert!(refused.contains("commits the remote doesn't"), "{refused}");
        assert_eq!(head(&clone_dir).unwrap(), mine);
    }

    /// A PATH with a planted `git-remote-evil` first on it, and the file that
    /// program leaves.
    fn planted_helper(dir: &Path) -> (String, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let helpers = dir.join("helpers");
        fs::create_dir_all(&helpers).unwrap();
        let ran = dir.join("ran");
        let helper = helpers.join("git-remote-evil");
        let text = format!("#!/bin/sh\ntouch '{}'\nexit 1\n", ran.display());
        fs::write(&helper, text).unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::var("PATH").unwrap_or_default();
        (format!("{}:{path}", helpers.display()), ran)
    }

    #[test]
    fn a_clones_url_cant_start_a_program_git_finds_by_its_name() {
        let dir = TempDir::new();
        let (path, ran) = planted_helper(dir.path());
        let mut command = clone_command("evil::x", None, &dir.path().join("clone"));
        command.env("PATH", path);
        assert!(run(command).is_err());
        assert!(!ran.exists());
    }

    #[test]
    fn a_fetchs_remote_cant_start_a_program_git_finds_by_its_name() {
        let dir = TempDir::new();
        let remote = repo(&dir.path().join("remote"), &[("alpha", "First")]);
        let clone_dir = dir.path().join("clone");
        clone(remote.to_str().unwrap(), None, &clone_dir).unwrap();
        in_repo(&clone_dir, &["remote", "set-url", "origin", "evil::x"]).unwrap();
        let (path, ran) = planted_helper(dir.path());
        let mut command = fetch_command(&clone_dir, None);
        command.env("PATH", path);
        assert!(run(command).is_err());
        assert!(!ran.exists());
    }

    #[test]
    fn a_branch_cant_be_an_option_to_the_fetch() {
        let dir = TempDir::new();
        let remote = repo(&dir.path().join("remote"), &[("alpha", "First")]);
        let clone_dir = dir.path().join("clone");
        clone(remote.to_str().unwrap(), None, &clone_dir).unwrap();
        let ran = dir.path().join("ran");
        let branch = format!("--upload-pack=touch {}", ran.display());
        assert!(update(&clone_dir, Some(&branch)).is_err());
        assert!(!ran.exists());
    }

    #[test]
    fn a_missing_remote_fails_in_git_s_words() {
        let dir = TempDir::new();
        let failed = clone(
            dir.path().join("nowhere").to_str().unwrap(),
            None,
            &dir.path().join("clone"),
        )
        .unwrap_err();
        assert!(failed.starts_with("git said:"), "{failed}");
    }

    #[test]
    fn bad_names_dot_folders_and_deep_folders_are_left_out() {
        let dir = TempDir::new();
        let root = dir.path().join("source");
        let skill = |path: &str, text: &str| {
            fs::create_dir_all(root.join(path)).unwrap();
            fs::write(root.join(path).join(SKILL_FILE), text).unwrap();
        };
        skill("good", "---\nname: good\ndescription: Fine\n---\n");
        skill("Bad Name", "no frontmatter");
        skill(".hidden/secret", "---\nname: secret\n---\n");
        skill("a/b/c/d/e/deep", "---\nname: deep\n---\n");
        skill("good/inner", "---\nname: inner\n---\n");
        let store = SkillStore::under(&dir.path().join("data"));

        let found = read(&root, SkillSourceId::derive("test"), &store);
        let names: Vec<_> = found.skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["good"]);
        assert_eq!(found.skipped.len(), 1, "{:?}", found.skipped);
        assert!(found.problem().unwrap().contains("Bad Name"));
        store.open_up();
    }
}
