// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Finds the repository a folder belongs to by reading `.git` directly. No
//! `git` process runs, so an observer can check every agent's folder on
//! every poll.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The most of a `.git` pointer, `commondir` or HEAD file that's read.
const SMALL: u64 = 16 * 1024;

/// The most of a repository's config that's read.
const CONFIG_MOST: u64 = 1024 * 1024;

/// A checkout found on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkout {
    /// The top folder of the checkout. For a linked worktree that's the
    /// worktree, not the main checkout.
    pub root: PathBuf,
    /// The URL of the `origin` remote, or of the first remote when there's
    /// no `origin`.
    pub remote: Option<String>,
}

/// The checkout that holds `folder`, if any.
pub fn find_checkout(folder: &Path) -> Option<Checkout> {
    folder.ancestors().find_map(checkout_at)
}

/// The checkout whose top folder is `root`, if `root` is the top of one.
pub fn checkout_at(root: &Path) -> Option<Checkout> {
    let common = common_dir(&root.join(".git"), root)?;
    let remote =
        read_small(&common.join("config"), CONFIG_MOST).and_then(|config| remote_url(&config));
    Some(Checkout {
        root: root.to_path_buf(),
        remote,
    })
}

/// The branch the checkout at `root` has checked out, read from its own
/// HEAD. None on a detached HEAD, or when `root` isn't the top of a
/// checkout.
pub fn head_branch(root: &Path) -> Option<String> {
    let head = read_small(&git_dir(&root.join(".git"), root)?.join("HEAD"), SMALL)?;
    head.trim()
        .strip_prefix("ref: refs/heads/")
        .map(str::to_owned)
}

/// The main checkout behind the linked worktree at `root`, or none when
/// `root` is a main checkout or no checkout at all.
pub fn main_checkout(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    if fs::metadata(&dot_git).ok()?.is_dir() {
        return None;
    }
    let common = fs::canonicalize(common_dir(&dot_git, root)?).ok()?;
    if common.file_name()? != ".git" {
        return None;
    }
    common.parent().map(Path::to_path_buf)
}

/// The checkout's own git dir, which holds its HEAD. A main checkout has
/// it in `.git`. A linked worktree has a `.git` file pointing at it.
fn git_dir(dot_git: &Path, checkout: &Path) -> Option<PathBuf> {
    let meta = fs::metadata(dot_git).ok()?;
    if meta.is_dir() {
        return Some(dot_git.to_path_buf());
    }
    let pointer = read_small(dot_git, SMALL)?;
    Some(checkout.join(pointer.strip_prefix("gitdir:")?.trim()))
}

/// The folder that holds the repository's config. A main checkout has it in
/// `.git`. A linked worktree's own git dir names the shared one in
/// `commondir`.
fn common_dir(dot_git: &Path, checkout: &Path) -> Option<PathBuf> {
    let git_dir = git_dir(dot_git, checkout)?;
    if git_dir == dot_git {
        return Some(git_dir);
    }
    match read_small(&git_dir.join("commondir"), SMALL) {
        Some(common) => Some(git_dir.join(common.trim())),
        None => Some(git_dir),
    }
}

/// The text of the plain file at `path`, when it holds at most `most`
/// bytes. A pipe, a device or a folder isn't opened, so a folder someone
/// planted one in can't hang whoever looks at it.
fn read_small(path: &Path, most: u64) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > most {
        return None;
    }
    let mut text = String::new();
    fs::File::open(path)
        .ok()?
        .take(most)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// The remote URL a git config names, preferring `origin`.
fn remote_url(config: &str) -> Option<String> {
    let mut section: Option<String> = None;
    let mut first = None;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with(';') || line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = header
                .strip_prefix("remote")
                .map(|name| name.trim().trim_matches('"').to_owned());
            continue;
        }
        let Some(remote) = &section else { continue };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("url") {
            continue;
        }
        let url = value.trim().trim_matches('"').to_owned();
        if remote == "origin" {
            return Some(url);
        }
        first.get_or_insert(url);
    }
    first
}

/// A remote URL reduced to host and path, lowercased and without `.git`, so
/// the SSH and HTTPS URLs of one repository match. None for a remote with
/// no host, such as a folder on the same disk.
///
/// `git@github.com:Example-User/MaxHQ.git` and `https://github.com/Example-User/MaxHQ`
/// both become `github.com/example-user/maxhq`.
pub fn normalize_remote(url: &str) -> Option<String> {
    let url = url.trim();
    let (host, path) = if let Some((_, rest)) = url.split_once("://") {
        if url.starts_with("file://") {
            return None;
        }
        rest.split_once('/')?
    } else {
        // The scp-like form, user@host:path. A Windows drive or a plain
        // path has no host before the colon.
        let (host, path) = url.split_once(':')?;
        if host.contains('/') || host.len() < 2 {
            return None;
        }
        (host, path)
    };
    let host = host.rsplit('@').next()?;
    let host = host.split(':').next()?;
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{host}/{path}").to_lowercase())
}

/// Whether `url` is a plain https link, the only kind a pill or a check may
/// open. Other schemes can start an app on this device.
pub fn is_https(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    scheme.eq_ignore_ascii_case("https")
        && !rest.is_empty()
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_https_link_counts() {
        assert!(is_https("https://github.com/owner/repo/pull/7"));
        assert!(is_https("HTTPS://github.com/owner/repo"));
        for url in [
            "",
            "https://",
            "https://a b",
            "https://a\nb",
            "http://github.com/owner/repo",
            "vscode://anysphere.cursor-deeplink/mcp/install?name=x",
            "file:///etc/hosts",
            "javascript:alert(1)",
            "//github.com/owner/repo",
        ] {
            assert!(!is_https(url), "{url}");
        }
    }

    #[test]
    fn ssh_and_https_urls_of_one_repo_match() {
        let want = Some("github.com/example-user/maxhq".to_owned());
        for url in [
            "git@github.com:Example-User/MaxHQ.git",
            "https://github.com/Example-User/MaxHQ",
            "https://max@github.com/Example-User/MaxHQ.git/",
            "ssh://git@github.com:22/Example-User/MaxHQ.git",
        ] {
            assert_eq!(normalize_remote(url), want, "{url}");
        }
    }

    #[test]
    fn local_remotes_have_no_host() {
        assert_eq!(normalize_remote("/srv/git/maxhq.git"), None);
        assert_eq!(normalize_remote("file:///srv/git/maxhq.git"), None);
        assert_eq!(normalize_remote("C:\\repos\\maxhq"), None);
    }

    #[test]
    fn origin_wins_over_other_remotes() {
        let config = r#"
[core]
	bare = false
[remote "upstream"]
	url = git@github.com:other/maxhq.git
[remote "origin"]
	url = git@github.com:Example-User/MaxHQ.git
	fetch = +refs/heads/*:refs/remotes/origin/*
"#;
        assert_eq!(
            remote_url(config).as_deref(),
            Some("git@github.com:Example-User/MaxHQ.git")
        );
        assert_eq!(
            remote_url("[remote \"fork\"]\n url = https://x.dev/a/b\n").as_deref(),
            Some("https://x.dev/a/b")
        );
        assert_eq!(remote_url("[core]\n url = nope\n"), None);
    }

    #[test]
    fn finds_main_checkouts_and_linked_worktrees() {
        let tmp = std::env::temp_dir().join(format!("maxhq-git-{}", std::process::id()));
        let main = tmp.join("main");
        let deep = main.join("src/deep");
        fs::create_dir_all(&deep).unwrap();
        fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
        fs::write(
            main.join(".git/config"),
            "[remote \"origin\"]\n\turl = git@github.com:Example-User/MaxHQ.git\n",
        )
        .unwrap();
        fs::write(main.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
        let wt = tmp.join("wt");
        fs::create_dir_all(&wt).unwrap();
        fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", main.join(".git/worktrees/wt").display()),
        )
        .unwrap();

        let found = find_checkout(&deep).unwrap();
        assert_eq!(found.root, main);
        assert_eq!(
            found.remote.as_deref(),
            Some("git@github.com:Example-User/MaxHQ.git")
        );
        let linked = find_checkout(&wt).unwrap();
        assert_eq!(linked.root, wt);
        assert_eq!(linked.remote, found.remote);
        assert_eq!(main_checkout(&wt), Some(fs::canonicalize(&main).unwrap()));
        assert_eq!(main_checkout(&main), None);
        assert_eq!(checkout_at(&main), Some(found));
        assert_eq!(checkout_at(&deep), None, "only the top folder counts");

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn reads_the_branch_from_head() {
        let tmp = std::env::temp_dir().join(format!("maxhq-git-head-{}", std::process::id()));
        let main = tmp.join("main");
        fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
        fs::write(main.join(".git/HEAD"), "ref: refs/heads/feat/login\n").unwrap();
        let wt = tmp.join("wt");
        fs::create_dir_all(&wt).unwrap();
        fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", main.join(".git/worktrees/wt").display()),
        )
        .unwrap();
        fs::write(main.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
        fs::write(
            main.join(".git/worktrees/wt/HEAD"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .unwrap();

        assert_eq!(head_branch(&main).as_deref(), Some("feat/login"));
        assert_eq!(
            head_branch(&wt),
            None,
            "the worktree's own HEAD is detached"
        );
        fs::write(main.join(".git/worktrees/wt/HEAD"), "ref: refs/heads/fix\n").unwrap();
        assert_eq!(head_branch(&wt).as_deref(), Some("fix"));
        assert_eq!(head_branch(&tmp), None, "not a checkout");

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn a_big_config_or_a_pipe_is_left_unread() {
        let tmp = std::env::temp_dir().join(format!("maxhq-git-pipe-{}", std::process::id()));
        let big = tmp.join("big");
        fs::create_dir_all(big.join(".git")).unwrap();
        let mut config = "[remote \"origin\"]\n\turl = git@github.com:owner/repo.git\n".to_owned();
        config.push_str(&"#".repeat(CONFIG_MOST as usize));
        fs::write(big.join(".git/config"), config).unwrap();
        fs::write(big.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(checkout_at(&big).map(|found| found.remote), Some(None));
        assert_eq!(head_branch(&big).as_deref(), Some("main"));

        let piped = tmp.join("piped");
        fs::create_dir_all(piped.join(".git")).unwrap();
        let pointed = tmp.join("pointed");
        fs::create_dir_all(&pointed).unwrap();
        for pipe in [
            piped.join(".git/HEAD"),
            piped.join(".git/config"),
            pointed.join(".git"),
        ] {
            let made = std::process::Command::new("mkfifo")
                .arg(&pipe)
                .status()
                .unwrap();
            assert!(made.success());
        }
        // Opening a pipe for reading waits for a writer, so look on
        // another thread and fail rather than hang.
        let (tx, rx) = std::sync::mpsc::channel();
        let (p, q) = (piped.clone(), pointed.clone());
        std::thread::spawn(move || {
            let _ = tx.send((
                checkout_at(&p),
                head_branch(&p),
                checkout_at(&q),
                head_branch(&q),
            ));
        });
        let found = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("a pipe hung the reader");
        let checkout = Some(Checkout {
            root: piped,
            remote: None,
        });
        assert_eq!(found, (checkout, None, None, None));

        fs::remove_dir_all(&tmp).unwrap();
    }
}
