// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Finds the repository a folder belongs to by reading `.git` directly. No
//! `git` process runs, so an observer can check every agent's folder on
//! every poll.

use std::fs;
use std::path::{Path, PathBuf};

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
    let mut dir = Some(folder);
    while let Some(current) = dir {
        let dot_git = current.join(".git");
        if let Some(common) = common_dir(&dot_git, current) {
            let remote = fs::read_to_string(common.join("config"))
                .ok()
                .and_then(|config| remote_url(&config));
            return Some(Checkout {
                root: current.to_path_buf(),
                remote,
            });
        }
        dir = current.parent();
    }
    None
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

/// The folder that holds the repository's config. A main checkout has it in
/// `.git`. A linked worktree has a `.git` file pointing at its own git dir,
/// which in turn names the shared one in `commondir`.
fn common_dir(dot_git: &Path, checkout: &Path) -> Option<PathBuf> {
    let meta = fs::metadata(dot_git).ok()?;
    if meta.is_dir() {
        return Some(dot_git.to_path_buf());
    }
    let pointer = fs::read_to_string(dot_git).ok()?;
    let git_dir = checkout.join(pointer.strip_prefix("gitdir:")?.trim());
    match fs::read_to_string(git_dir.join("commondir")) {
        Ok(common) => Some(git_dir.join(common.trim())),
        Err(_) => Some(git_dir),
    }
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

        fs::remove_dir_all(&tmp).unwrap();
    }
}
