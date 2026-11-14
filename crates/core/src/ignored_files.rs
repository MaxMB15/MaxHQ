// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: listing the
// ignored files, translated to Rust. Leaving out paths under `.git`
// and paths that aren't UTF-8 is MaxHQ's.

//! The files git ignores in a checkout, such as `.env`, for copying a
//! project to another device. `--directory` reports a folder holding
//! nothing but ignored files as one `name/` entry, which is how
//! `node_modules` and build output drop out, and so does a folder git
//! doesn't track at all. A listing git can't make is empty.

use std::io::Read;
use std::path::{Component, Path};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use maxhq_protocol::message::IgnoredFile;

/// The most files a listing holds.
pub const IGNORED_FILES_LIMIT: usize = 200;

/// How long git may take to list.
pub const LIST_TIMEOUT: Duration = Duration::from_secs(10);

/// The most bytes of git's output read. A listing over it is empty.
pub const MAX_OUTPUT: usize = 4 * 1024 * 1024;

/// What a listing found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ignored {
    /// Regular files, sorted by path, at most [`IGNORED_FILES_LIMIT`].
    pub files: Vec<IgnoredFile>,
    /// Whether git listed more than the limit.
    pub truncated: bool,
}

/// Lists the ignored files in `checkout`. `git` runs git as whoever the
/// caller picked, with the environment it set. This blocks for up to
/// [`LIST_TIMEOUT`].
pub fn list(mut git: Command, checkout: &Path) -> Ignored {
    git.args([
        "-c",
        "core.fsmonitor=false",
        "ls-files",
        "--others",
        "--ignored",
        "--exclude-standard",
        "--directory",
        "--no-empty-directory",
        "-z",
    ])
    .current_dir(checkout);
    match run(git) {
        Some(stdout) => from_output(&stdout, checkout),
        None => Ignored::default(),
    }
}

/// Runs git and returns its output, or None when it fails, takes too long
/// or says too much.
fn run(mut git: Command) -> Option<Vec<u8>> {
    let mut child = git
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut out = Vec::new();
        let read = stdout.take(MAX_OUTPUT as u64 + 1).read_to_end(&mut out);
        let _ = tx.send(read.ok().filter(|&len| len <= MAX_OUTPUT).map(|_| out));
    });
    let output = rx.recv_timeout(LIST_TIMEOUT).ok().flatten();
    if output.is_none() {
        let _ = child.kill();
    }
    let status = child.wait().ok()?;
    output.filter(|_| status.success())
}

/// Reads git's NUL-separated paths, and keeps the regular files.
fn from_output(stdout: &[u8], checkout: &Path) -> Ignored {
    let mut paths: Vec<&str> = stdout
        .split(|&byte| byte == 0)
        .filter_map(|entry| std::str::from_utf8(entry).ok())
        .filter(|entry| !entry.is_empty() && !entry.ends_with('/'))
        .collect();
    paths.sort_unstable();
    let files = paths
        .iter()
        .take(IGNORED_FILES_LIMIT)
        .filter(|path| !in_git_dir(Path::new(path)))
        .filter_map(|path| {
            // lstat: a symlink usually points outside the checkout, where
            // reads refuse to go.
            let meta = checkout.join(path).symlink_metadata().ok()?;
            meta.is_file().then(|| IgnoredFile {
                path: (*path).to_owned(),
                size: meta.len(),
            })
        })
        .collect();
    Ignored {
        files,
        truncated: paths.len() > IGNORED_FILES_LIMIT,
    }
}

/// Whether a path has a `.git` part, such as a nested repo's.
fn in_git_dir(path: &Path) -> bool {
    path.components()
        .any(|part| matches!(part, Component::Normal(name) if name == ".git"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::test_dir::TempDir;

    fn git() -> Command {
        let mut git = Command::new("git");
        git.env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE");
        git
    }

    /// A fresh repo, or None when this machine has no git.
    fn repo() -> Option<TempDir> {
        let dir = TempDir::new("ignored");
        let status = git()
            .args(["init", "-q"])
            .current_dir(&dir.0)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        status.success().then_some(dir)
    }

    fn write(root: &Path, path: &str, bytes: &[u8]) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// Adds what isn't ignored to the index, as a checkout's tracked files.
    fn track(root: &Path) {
        let status = git()
            .args(["add", "-A"])
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn paths(found: &Ignored) -> Vec<&str> {
        found.files.iter().map(|file| file.path.as_str()).collect()
    }

    #[test]
    fn lists_ignored_files_and_collapses_ignored_folders() {
        let Some(dir) = repo() else { return };
        let root = &dir.0;
        write(root, ".gitignore", b".env*\nnode_modules/\n*.key\n");
        write(root, ".env", b"TOKEN=1\n");
        write(root, "config/app.toml", b"port = 3000\n");
        write(root, "config/.env.local", b"PORT=3000\n");
        write(
            root,
            "node_modules/left-pad/index.js",
            b"module.exports = 1;\n",
        );
        write(root, "src/main.rs", b"fn main() {}\n");
        write(root, "nested/.git/secret.key", b"inside a nested repo\n");
        std::os::unix::fs::symlink("/etc/hosts", root.join(".env.link")).unwrap();
        track(root);

        let found = list(git(), root);
        assert_eq!(paths(&found), [".env", "config/.env.local"]);
        assert_eq!(found.files[0].size, 8);
        assert!(!found.truncated);
    }

    #[test]
    fn stops_at_the_limit() {
        let Some(dir) = repo() else { return };
        let root = &dir.0;
        write(root, ".gitignore", b"*.log\n");
        write(root, "kept.txt", b"tracked\n");
        for n in 0..IGNORED_FILES_LIMIT + 5 {
            write(root, &format!("logs/{n:03}.log"), b"x");
        }
        // The folder holds a tracked file, so it doesn't collapse.
        write(root, "logs/README", b"not ignored\n");
        track(root);

        let found = list(git(), root);
        assert_eq!(found.files.len(), IGNORED_FILES_LIMIT);
        assert_eq!(found.files[0].path, "logs/000.log");
        assert!(found.truncated);
    }

    #[test]
    fn output_over_the_cap_lists_nothing() {
        let dir = TempDir::new("ignored");
        let mut entry = vec![b'a'; 1000];
        entry.push(0);
        let output = entry.repeat(MAX_OUTPUT / entry.len() + 1);
        write(&dir.0, "out", &output);
        let mut cat = Command::new("cat");
        cat.arg(dir.0.join("out"));
        assert_eq!(run(cat), None);

        let mut short = Command::new("cat");
        short.arg(dir.0.join("out"));
        fs::write(dir.0.join("out"), b"a\0").unwrap();
        assert_eq!(run(short).as_deref(), Some(&b"a\0"[..]));
    }

    #[test]
    fn a_folder_git_refuses_lists_nothing() {
        let dir = TempDir::new("ignored");
        assert_eq!(list(git(), &dir.0), Ignored::default());
    }
}
