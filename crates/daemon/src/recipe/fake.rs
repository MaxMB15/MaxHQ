// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A fake container for the recipe's tests. [`FakeExec`] keeps files,
//! folders, packages, users and units in memory, records every call, and
//! runs the commands the steps use the way Debian 13 would, as far as the
//! steps can tell. The vendor installers never run: their scripts are
//! matched by text and their effects made up, and so are the installer's
//! own scripts. Downloads come from a table of fake releases.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard, PoisonError};

use futures::future::{self, BoxFuture, FutureExt};

use super::retire::LEFT;
use super::steps::{CODEX_BUILDS, CODEX_RELEASE, CURSOR_DOWNLOADS, CURSOR_VERSION, PACKAGES};
use super::{Exec, ExecError, ExecOutput, ExecUser, AGENT_UID};
use crate::tools::install::fetch::{Fetched, Progress};
use crate::tools::install::scripts::{
    CHECK, FINISH, LOCK, PUBLISH, REMOVE, RUN, STAGE, UNLOCK, UNPACK,
};

/// The Claude version the fake installer installs.
pub const CLAUDE_VERSION: &str = "2.1.211";
/// The Codex version the fake release holds.
pub const CODEX_VERSION: &str = "0.164.0";

/// What a fake binary's file starts with, followed by its name and version.
const BINARY: &str = "fake-binary";
/// What a fake archive starts with, followed by a line per file in it: its
/// path, a tab and what it holds.
const ARCHIVE: &str = "fake-archive";
/// Claude's install script, as the fake serves it.
pub const CLAUDE_SCRIPT: &str = "fake claude installer\n";
/// Where Claude's install script comes from.
pub const CLAUDE_SCRIPT_URL: &str = "https://claude.ai/install.sh";

/// The start of Debian's `/etc/skel/.bashrc`, which `useradd` copies.
pub const SKEL_BASHRC: &str = "\
# ~/.bashrc: executed by bash(1) for non-login shells.
# see /usr/share/doc/bash/examples/startup-files (in the package bash-doc)
# for examples

# If not running interactively, don't do anything
case $- in
    *i*) ;;
      *) return;;
esac

# don't put duplicate lines or lines starting with space in the history.
HISTCONTROL=ignoreboth
";

/// One call the recipe made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    /// A command, with every word as given.
    Run {
        /// The command.
        cmd: Vec<String>,
        /// Who it ran as.
        user: ExecUser,
    },
    /// A file read.
    Read(String),
    /// A download into the container.
    Download {
        /// What was fetched.
        url: String,
        /// Where it went.
        path: String,
        /// Who wrote it.
        user: ExecUser,
    },
    /// A file write.
    Write {
        /// The file.
        path: String,
        /// Its mode.
        mode: u32,
        /// Its owner.
        owner: ExecUser,
    },
}

impl Call {
    /// The command's words after any `env` settings, or `None` for a file
    /// call.
    pub fn command(&self) -> Option<Vec<&str>> {
        match self {
            Call::Run { cmd, .. } => {
                let words: Vec<&str> = cmd.iter().map(String::as_str).collect();
                Some(strip_env(&words).1.to_vec())
            }
            _ => None,
        }
    }
}

/// A file in the fake.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    /// What it holds.
    pub bytes: Vec<u8>,
    /// Its mode.
    pub mode: u32,
    /// Its owner's uid.
    pub uid: u32,
    /// Its group's gid.
    pub gid: u32,
}

impl File {
    /// What it holds, as text.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

/// A folder in the fake.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dir {
    /// Its mode.
    pub mode: u32,
    /// Its owner's uid.
    pub uid: u32,
    /// Its group's gid.
    pub gid: u32,
}

/// A line of `/etc/passwd`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    /// The name.
    pub name: String,
    /// The uid.
    pub uid: u32,
    /// The primary gid.
    pub gid: u32,
    /// The home folder.
    pub home: String,
    /// The login shell.
    pub shell: String,
}

/// A file the fake network serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Download {
    /// The SHA-256 it arrives with.
    pub sha256: String,
    /// What it holds.
    pub body: String,
}

impl Download {
    /// An archive holding `files`, as paths and what they hold.
    fn archive(sha256: &str, files: &[(String, String)]) -> Self {
        let mut body = format!("{ARCHIVE}\n");
        for (path, text) in files {
            body.push_str(&format!("{path}\t{text}\n"));
        }
        Download {
            sha256: sha256.to_owned(),
            body,
        }
    }
}

/// A canned answer for commands that start with some words.
enum Answer {
    Output(ExecOutput),
    Transport,
}

/// Everything in the fake container.
#[derive(Default)]
pub struct Container {
    /// Files by path.
    pub files: BTreeMap<String, File>,
    /// Folders by path.
    pub dirs: BTreeMap<String, Dir>,
    /// Symbolic links by path, to what they hold.
    pub links: BTreeMap<String, String>,
    /// Installed packages.
    pub packages: BTreeSet<String>,
    /// True once `apt-get update` has run.
    pub lists: bool,
    /// `/etc/passwd`.
    pub users: Vec<User>,
    /// `/etc/group`, as names and gids.
    pub groups: Vec<(String, u32)>,
    /// What `uname -m` prints.
    pub machine: String,
    /// The unit files systemd last loaded, by unit name.
    pub loaded: BTreeMap<String, Vec<u8>>,
    /// Enabled units.
    pub enabled: BTreeSet<String>,
    /// How many times each unit restarted.
    pub restarts: BTreeMap<String, u32>,
    /// What the network serves, by URL.
    pub downloads: BTreeMap<String, Download>,
    /// Every call, in order.
    pub calls: Vec<Call>,
    answers: Vec<(Vec<String>, Answer)>,
}

/// A fake Debian 13 container.
pub struct FakeExec {
    state: Mutex<Container>,
}

impl FakeExec {
    /// A container fresh from the Debian 13 image: root, the base folders,
    /// and none of the recipe's packages.
    pub fn debian() -> FakeExec {
        let mut state = Container {
            machine: "x86_64".to_owned(),
            users: vec![User {
                name: "root".to_owned(),
                uid: 0,
                gid: 0,
                home: "/root".to_owned(),
                shell: "/bin/bash".to_owned(),
            }],
            groups: vec![("root".to_owned(), 0)],
            ..Container::default()
        };
        for dir in [
            "/",
            "/etc",
            "/etc/systemd",
            "/etc/systemd/system",
            "/home",
            "/root",
            "/tmp",
            "/usr",
            "/usr/bin",
            "/usr/local",
            "/usr/local/bin",
        ] {
            let mode = if dir == "/root" { 0o700 } else { 0o755 };
            state.dirs.insert(dir.to_owned(), dir_of(mode, 0));
        }
        for build in CODEX_BUILDS {
            for (name, sha256) in [
                ("codex", build.sha256),
                ("codex-code-mode-host", build.host_sha256),
            ] {
                let member = format!("{name}-{}", build.target);
                state.downloads.insert(
                    format!("{CODEX_RELEASE}/{member}.tar.gz"),
                    Download::archive(
                        sha256,
                        &[(member.clone(), format!("{BINARY} {name} {CODEX_VERSION}"))],
                    ),
                );
            }
        }
        for arch in ["x64", "arm64"] {
            state.downloads.insert(
                format!(
                    "{CURSOR_DOWNLOADS}/{CURSOR_VERSION}/linux/{arch}/agent-cli-package.tar.gz"
                ),
                Download::archive(
                    &"c".repeat(64),
                    &[
                        (
                            "dist-package/cursor-agent".to_owned(),
                            format!("{BINARY} cursor-agent {CURSOR_VERSION}"),
                        ),
                        (
                            "dist-package/node".to_owned(),
                            format!("{BINARY} node 22.0.0"),
                        ),
                    ],
                ),
            );
        }
        state.downloads.insert(
            CLAUDE_SCRIPT_URL.to_owned(),
            Download {
                sha256: "a".repeat(64),
                body: CLAUDE_SCRIPT.to_owned(),
            },
        );
        FakeExec {
            state: Mutex::new(state),
        }
    }

    /// The container, to look at or change.
    pub fn lock(&self) -> MutexGuard<'_, Container> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Makes every command that starts with `prefix`, after any `env`
    /// settings, print and exit as `output` says, without doing anything.
    pub fn answer(&self, prefix: &[&str], output: ExecOutput) {
        let prefix = prefix.iter().map(|word| (*word).to_owned()).collect();
        self.lock().answers.push((prefix, Answer::Output(output)));
    }

    /// Makes every command that starts with `prefix` fail to reach the
    /// container.
    pub fn cut(&self, prefix: &[&str]) {
        let prefix = prefix.iter().map(|word| (*word).to_owned()).collect();
        self.lock().answers.push((prefix, Answer::Transport));
    }

    /// A file, if one is there, through any links.
    pub fn file(&self, path: &str) -> Option<File> {
        let state = self.lock();
        state.files.get(&state.resolve(path)).cloned()
    }

    /// Where `path` leads, through any links.
    pub fn resolve(&self, path: &str) -> String {
        self.lock().resolve(path)
    }

    /// Puts a fake binary at `path` that reports `version`, in place of
    /// any link there.
    pub fn put_binary(&self, path: &str, name: &str, version: &str, uid: u32) {
        let bytes = format!("{BINARY} {name} {version}\n").into_bytes();
        let mut state = self.lock();
        state.links.remove(path);
        state.files.insert(
            path.to_owned(),
            File {
                bytes,
                mode: 0o755,
                uid,
                gid: uid,
            },
        );
    }

    /// How many calls the recipe has made.
    pub fn call_count(&self) -> usize {
        self.lock().calls.len()
    }

    /// The calls since the first `from`.
    pub fn calls_since(&self, from: usize) -> Vec<Call> {
        self.lock().calls[from..].to_vec()
    }
}

impl Exec for FakeExec {
    fn run<'a>(
        &'a self,
        cmd: &'a [&'a str],
        user: ExecUser,
        _stdin: Option<&'a [u8]>,
    ) -> BoxFuture<'a, Result<ExecOutput, ExecError>> {
        let result = self.lock().run(cmd, user);
        future::ready(result).boxed()
    }

    fn read<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Option<Vec<u8>>, ExecError>> {
        let result = self.lock().read(path);
        future::ready(result).boxed()
    }

    fn read_as_agent<'a>(
        &'a self,
        path: &'a str,
    ) -> BoxFuture<'a, Result<Option<Vec<u8>>, ExecError>> {
        let result = self.lock().read_as_agent(path);
        future::ready(result).boxed()
    }

    fn write<'a>(
        &'a self,
        path: &'a str,
        bytes: &'a [u8],
        mode: u32,
        owner: ExecUser,
    ) -> BoxFuture<'a, Result<(), ExecError>> {
        let result = self.lock().write(path, bytes, mode, owner);
        future::ready(result).boxed()
    }

    fn download<'a>(
        &'a self,
        url: &'a str,
        path: &'a str,
        user: ExecUser,
        _most: Option<u64>,
        _progress: Progress<'a>,
    ) -> BoxFuture<'a, Result<Fetched, ExecError>> {
        let result = self.lock().download(url, path, user);
        future::ready(result).boxed()
    }
}

fn dir_of(mode: u32, uid: u32) -> Dir {
    Dir {
        mode,
        uid,
        gid: uid,
    }
}

fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(at) => &path[..at],
        None => "",
    }
}

/// Splits `env NAME=VALUE ...` from the command after it.
fn strip_env<'a, 'w>(cmd: &'a [&'w str]) -> (BTreeMap<&'w str, &'w str>, &'a [&'w str]) {
    let mut env = BTreeMap::new();
    let mut rest = cmd;
    if rest.first() == Some(&"env") {
        rest = &rest[1..];
        while let Some((name, value)) = rest.first().and_then(|word| word.split_once('=')) {
            env.insert(name, value);
            rest = &rest[1..];
        }
    }
    (env, rest)
}

fn out(stdout: impl Into<String>) -> ExecOutput {
    ExecOutput {
        status: 0,
        stdout: stdout.into().into_bytes(),
        stderr: Vec::new(),
    }
}

fn exit(status: i32, stderr: impl Into<String>) -> ExecOutput {
    ExecOutput {
        status,
        stdout: Vec::new(),
        stderr: stderr.into().into_bytes(),
    }
}

/// The value after `flag` in `args`.
fn flag<'w>(args: &[&'w str], flag: &str) -> Option<&'w str> {
    let at = args.iter().position(|arg| *arg == flag)?;
    args.get(at + 1).copied()
}

impl Container {
    fn run(&mut self, cmd: &[&str], user: ExecUser) -> Result<ExecOutput, ExecError> {
        self.calls.push(Call::Run {
            cmd: cmd.iter().map(|word| (*word).to_owned()).collect(),
            user,
        });
        let (env, words) = strip_env(cmd);
        let answer = self.answers.iter().find(|(prefix, _)| {
            prefix.len() <= words.len() && prefix.iter().zip(words).all(|(a, b)| a == b)
        });
        match answer {
            Some((_, Answer::Output(output))) => return Ok(output.clone()),
            Some((_, Answer::Transport)) => {
                return Err(ExecError::Transport("the link dropped".to_owned()))
            }
            None => {}
        }
        let Some((program, args)) = words.split_first() else {
            return Err(ExecError::Transport("an empty command".to_owned()));
        };
        let root = user == ExecUser::Root;
        let output = match *program {
            "dpkg-query" => self.dpkg_query(args),
            "apt-get" if !root => exit(
                100,
                "E: Could not open lock file - open (13: Permission denied)",
            ),
            "apt-get" => self.apt_get(args, &env),
            "getent" => self.getent(args),
            "groupadd" if !root => exit(10, "groupadd: Permission denied."),
            "groupadd" => self.groupadd(args),
            "useradd" if !root => exit(1, "useradd: Permission denied."),
            "useradd" => self.useradd(args),
            "install" if !root => exit(1, "install: cannot change owner: Operation not permitted"),
            "install" => self.install_dir(args),
            "stat" => self.stat(args),
            "sh" => return self.sh(args, user, &env),
            "rm" if args.first() == Some(&"-rf") => self.rm_rf(&args[1..], user.uid()),
            "uname" if args == ["-m"] => out(format!("{}\n", self.machine)),
            "systemctl" => self.systemctl(args, root),
            path if path.starts_with('/') => self.binary(path, args),
            _ => {
                return Err(ExecError::Transport(format!(
                    "the fake can't run {program}"
                )))
            }
        };
        Ok(output)
    }

    fn read(&mut self, path: &str) -> Result<Option<Vec<u8>>, ExecError> {
        self.calls.push(Call::Read(path.to_owned()));
        let path = self.resolve(path);
        if self.dirs.contains_key(&path) {
            return Err(ExecError::Transport(format!("{path} is a folder")));
        }
        Ok(self.files.get(&path).map(|file| file.bytes.clone()))
    }

    /// Fails as `cat` would on a file only someone else can read.
    fn read_as_agent(&mut self, path: &str) -> Result<Option<Vec<u8>>, ExecError> {
        let to = self.resolve(path);
        if let Some(file) = self.files.get(&to)
            && file.uid != AGENT_UID
            && file.mode & 0o004 == 0
        {
            let cmd = ["cat", path];
            return Err(ExecError::failed(&cmd, &exit(1, "Permission denied")));
        }
        self.read(path)
    }

    fn write(
        &mut self,
        path: &str,
        bytes: &[u8],
        mode: u32,
        owner: ExecUser,
    ) -> Result<(), ExecError> {
        self.calls.push(Call::Write {
            path: path.to_owned(),
            mode,
            owner,
        });
        if !self.dirs.contains_key(parent(path)) {
            return Err(ExecError::Transport(format!("no folder for {path}")));
        }
        if !self.writable(parent(path), owner.uid()) {
            let cmd = ["sh", "-c", "write", path];
            return Err(ExecError::failed(&cmd, &exit(1, "Permission denied")));
        }
        if self.dirs.contains_key(path) {
            return Err(ExecError::Transport(format!("{path} is a folder")));
        }
        self.files.insert(
            path.to_owned(),
            File {
                bytes: bytes.to_vec(),
                mode,
                uid: owner.uid(),
                gid: owner.uid(),
            },
        );
        Ok(())
    }

    fn dpkg_query(&self, args: &[&str]) -> ExecOutput {
        let [show, format, packages @ ..] = args else {
            return exit(2, "dpkg-query: need an action option");
        };
        if *show != "--show" || !format.starts_with("--showformat=") {
            return exit(2, "dpkg-query: unknown option");
        }
        let mut output = out("");
        for package in packages {
            if self.packages.contains(*package) {
                output
                    .stdout
                    .extend(format!("{package} installed\n").into_bytes());
            } else {
                output.status = 1;
                output.stderr.extend(
                    format!("dpkg-query: no packages found matching {package}\n").into_bytes(),
                );
            }
        }
        output
    }

    fn apt_get(&mut self, args: &[&str], env: &BTreeMap<&str, &str>) -> ExecOutput {
        match args {
            ["update", "-q"] => {
                self.lists = true;
                out("Reading package lists...\n")
            }
            ["install", "-y", "-q", packages @ ..] => {
                if env.get("DEBIAN_FRONTEND") != Some(&"noninteractive") {
                    return exit(100, "debconf: unable to initialize frontend: Dialog");
                }
                for package in packages {
                    if !self.lists || !PACKAGES.contains(package) {
                        return exit(100, format!("E: Unable to locate package {package}"));
                    }
                }
                for package in packages {
                    self.packages.insert((*package).to_owned());
                    let binary = match *package {
                        "gh" => Some(("/usr/bin/gh", "2.46.0")),
                        "curl" => Some(("/usr/bin/curl", "8.14.1")),
                        "git" => Some(("/usr/bin/git", "2.47.3")),
                        _ => None,
                    };
                    if let Some((path, version)) = binary {
                        let name = file_name(path);
                        self.files.insert(
                            path.to_owned(),
                            File {
                                bytes: format!("{BINARY} {name} {version}\n").into_bytes(),
                                mode: 0o755,
                                uid: 0,
                                gid: 0,
                            },
                        );
                    }
                }
                out("")
            }
            _ => exit(100, "E: Invalid operation"),
        }
    }

    fn getent(&self, args: &[&str]) -> ExecOutput {
        let [database, key] = args else {
            return exit(1, "getent: wrong arguments");
        };
        let id: Option<u32> = key.parse().ok();
        match *database {
            "passwd" => self
                .users
                .iter()
                .find(|user| Some(user.uid) == id || user.name == *key)
                .map(|user| {
                    out(format!(
                        "{}:x:{}:{}::{}:{}\n",
                        user.name, user.uid, user.gid, user.home, user.shell
                    ))
                })
                .unwrap_or_else(|| exit(2, "")),
            "group" => self
                .groups
                .iter()
                .find(|(name, gid)| Some(*gid) == id || name == key)
                .map(|(name, gid)| out(format!("{name}:x:{gid}:\n")))
                .unwrap_or_else(|| exit(2, "")),
            _ => exit(1, "getent: Unknown database"),
        }
    }

    fn groupadd(&mut self, args: &[&str]) -> ExecOutput {
        let (Some(gid), Some(name)) = (
            flag(args, "--gid").and_then(|gid| gid.parse::<u32>().ok()),
            args.last(),
        ) else {
            return exit(2, "groupadd: invalid arguments");
        };
        if self.groups.iter().any(|(_, used)| *used == gid) {
            return exit(4, format!("groupadd: GID '{gid}' already exists"));
        }
        if self.groups.iter().any(|(used, _)| used == name) {
            return exit(9, format!("groupadd: group '{name}' already exists"));
        }
        self.groups.push(((*name).to_owned(), gid));
        out("")
    }

    fn useradd(&mut self, args: &[&str]) -> ExecOutput {
        let uid = flag(args, "--uid").and_then(|uid| uid.parse::<u32>().ok());
        let gid = flag(args, "--gid").and_then(|gid| gid.parse::<u32>().ok());
        let (Some(uid), Some(gid), Some(home), Some(shell), Some(name)) = (
            uid,
            gid,
            flag(args, "--home-dir"),
            flag(args, "--shell"),
            args.last(),
        ) else {
            return exit(2, "useradd: invalid arguments");
        };
        if self.users.iter().any(|user| user.uid == uid) {
            return exit(4, format!("useradd: UID {uid} is not unique"));
        }
        if self.users.iter().any(|user| user.name == *name) {
            return exit(9, format!("useradd: user '{name}' already exists"));
        }
        if !self.groups.iter().any(|(_, used)| *used == gid) {
            return exit(6, format!("useradd: group '{gid}' does not exist"));
        }
        self.users.push(User {
            name: (*name).to_owned(),
            uid,
            gid,
            home: home.to_owned(),
            shell: shell.to_owned(),
        });
        if args.contains(&"--create-home") {
            if !self.dirs.contains_key(parent(home)) {
                return exit(12, format!("useradd: cannot create directory {home}"));
            }
            self.dirs.insert(
                home.to_owned(),
                Dir {
                    mode: 0o700,
                    uid,
                    gid,
                },
            );
            for (name, text) in [
                (".bashrc", SKEL_BASHRC),
                (
                    ".profile",
                    "# ~/.profile: executed by the command interpreter for login shells.\n",
                ),
                (
                    ".bash_logout",
                    "# ~/.bash_logout: executed by bash(1) when login shell exits.\n",
                ),
            ] {
                self.files.insert(
                    format!("{home}/{name}"),
                    File {
                        bytes: text.as_bytes().to_vec(),
                        mode: 0o644,
                        uid,
                        gid,
                    },
                );
            }
        }
        out("")
    }

    /// `install -d -o UID -g GID -m MODE PATH`, which makes missing
    /// parents and sets the last folder's owner and mode even when it's
    /// already there.
    fn install_dir(&mut self, args: &[&str]) -> ExecOutput {
        let (Some(uid), Some(gid), Some(mode), Some(path)) = (
            flag(args, "-o").and_then(|uid| uid.parse::<u32>().ok()),
            flag(args, "-g").and_then(|gid| gid.parse::<u32>().ok()),
            flag(args, "-m").and_then(|mode| u32::from_str_radix(mode, 8).ok()),
            args.last(),
        ) else {
            return exit(1, "install: invalid arguments");
        };
        if args.first() != Some(&"-d") {
            return exit(1, "install: the fake only makes folders");
        }
        let mut at = String::new();
        for part in path.split('/').filter(|part| !part.is_empty()) {
            at.push('/');
            at.push_str(part);
            if self.files.contains_key(&at) {
                return exit(
                    1,
                    format!("install: cannot create directory '{at}': File exists"),
                );
            }
            self.dirs
                .entry(at.clone())
                .or_insert_with(|| dir_of(0o755, 0));
        }
        self.dirs.insert((*path).to_owned(), Dir { mode, uid, gid });
        out("")
    }

    fn stat(&self, args: &[&str]) -> ExecOutput {
        let (follow, format, path) = match args {
            ["-c", format, path] => (false, *format, (*path).to_owned()),
            ["-L", "-c", format, path] => (true, *format, self.resolve(path)),
            _ => return exit(1, "stat: invalid arguments"),
        };
        let path = path.as_str();
        let (mode, uid, gid, kind) = if !follow && self.links.contains_key(path) {
            (0o777, 0, 0, "symbolic link")
        } else if let Some(dir) = self.dirs.get(path) {
            (dir.mode, dir.uid, dir.gid, "directory")
        } else if let Some(file) = self.files.get(path) {
            let kind = if file.bytes.is_empty() {
                "regular empty file"
            } else {
                "regular file"
            };
            (file.mode, file.uid, file.gid, kind)
        } else {
            return exit(
                1,
                format!("stat: cannot statx '{path}': No such file or directory"),
            );
        };
        let text = format
            .replace("%a", &format!("{mode:o}"))
            .replace("%u", &uid.to_string())
            .replace("%g", &gid.to_string())
            .replace("%F", kind);
        out(format!("{text}\n"))
    }

    fn systemctl(&mut self, args: &[&str], root: bool) -> ExecOutput {
        let unit_path = |unit: &str| format!("/etc/systemd/system/{unit}");
        match args {
            ["is-enabled", unit] => {
                if self.enabled.contains(*unit) {
                    out("enabled\n")
                } else if self.files.contains_key(&unit_path(unit)) {
                    ExecOutput {
                        status: 1,
                        stdout: b"disabled\n".to_vec(),
                        stderr: Vec::new(),
                    }
                } else {
                    exit(
                        1,
                        format!(
                            "Failed to get unit file state for {unit}: No such file or directory"
                        ),
                    )
                }
            }
            _ if !root => exit(1, "Failed: Access denied"),
            ["daemon-reload"] => {
                self.loaded = self
                    .files
                    .iter()
                    .filter(|(path, _)| parent(path) == "/etc/systemd/system")
                    .map(|(path, file)| (file_name(path).to_owned(), file.bytes.clone()))
                    .collect();
                out("")
            }
            ["enable", unit] => match self.files.get(&unit_path(unit)) {
                None => exit(
                    1,
                    format!("Failed to enable unit: Unit file {unit} does not exist."),
                ),
                Some(file) if !file.text().contains("[Install]") => {
                    exit(1, "The unit files have no installation config.")
                }
                Some(_) => {
                    self.enabled.insert((*unit).to_owned());
                    out("")
                }
            },
            ["restart", unit] => {
                let on_disk = self.files.get(&unit_path(unit)).map(|file| &file.bytes);
                match self.loaded.get(*unit) {
                    None => exit(
                        5,
                        format!("Failed to restart {unit}: Unit {unit} not found."),
                    ),
                    // Real systemd restarts the old unit with a warning.
                    // The fake fails, so a missing daemon-reload shows.
                    Some(loaded) if Some(loaded) != on_disk => exit(
                        1,
                        format!("Warning: The unit file of {unit} changed on disk."),
                    ),
                    Some(_) => {
                        *self.restarts.entry((*unit).to_owned()).or_default() += 1;
                        out("")
                    }
                }
            }
            _ => exit(1, "systemctl: unknown command"),
        }
    }

    /// What Claude's installer leaves: a version under
    /// `~/.local/share/claude` and the launcher in `~/.local/bin`.
    fn install_claude(&mut self, user: ExecUser, env: &BTreeMap<&str, &str>) -> ExecOutput {
        // Without HOME, Incus exec leaves root's.
        let home = env.get("HOME").copied().unwrap_or("/root");
        let uid = user.uid();
        match self.dirs.get(home) {
            Some(dir) if dir.uid == uid => {}
            _ => {
                return exit(
                    1,
                    format!("mkdir: cannot create directory '{home}/.local': Permission denied"),
                )
            }
        }
        for dir in [
            ".local",
            ".local/bin",
            ".local/share",
            ".local/share/claude",
            ".local/share/claude/versions",
        ] {
            self.dirs
                .entry(format!("{home}/{dir}"))
                .or_insert_with(|| dir_of(0o755, uid));
        }
        let binary = File {
            bytes: format!("{BINARY} claude {CLAUDE_VERSION}\n").into_bytes(),
            mode: 0o755,
            uid,
            gid: uid,
        };
        self.files.insert(
            format!("{home}/.local/share/claude/versions/{CLAUDE_VERSION}"),
            binary.clone(),
        );
        self.files
            .insert(format!("{home}/.local/bin/claude"), binary);
        out("Claude Code successfully installed!\n")
    }

    /// Runs a fake binary by path. Only `--version` does anything.
    fn binary(&self, path: &str, args: &[&str]) -> ExecOutput {
        let Some(file) = self.files.get(&self.resolve(path)) else {
            return exit(127, format!("env: '{path}': No such file or directory"));
        };
        if file.mode & 0o111 == 0 {
            return exit(126, format!("env: '{path}': Permission denied"));
        }
        let text = file.text();
        let mut words = text.split_whitespace();
        let (Some(BINARY), Some(name), Some(version)) = (words.next(), words.next(), words.next())
        else {
            return exit(126, format!("env: '{path}': Exec format error"));
        };
        match (name, args) {
            ("claude", ["--version"]) => out(format!("{version} (Claude Code)\n")),
            ("codex", ["--version"]) => out(format!("codex-cli {version}\n")),
            ("cursor-agent", ["--version"]) => out(format!("{version}\n")),
            _ => exit(2, format!("{name}: the fake knows only --version")),
        }
    }
}

/// The last part of a path.
fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `path` with empty and `.` parts dropped.
fn tidy(path: &str) -> String {
    let parts: Vec<&str> = path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    format!("/{}", parts.join("/"))
}

/// True when `path` is `root` or inside it.
fn within(path: &str, root: &str) -> bool {
    path == root || path.starts_with(&format!("{root}/"))
}

/// The installer's scripts, as the fake runs them, and the files they move.
impl Container {
    /// Where `path` leads, through links in any part of it.
    fn resolve(&self, path: &str) -> String {
        let mut path = tidy(path);
        for _ in 0..16 {
            let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
            let mut at = String::new();
            let mut next = None;
            for (index, part) in parts.iter().enumerate() {
                at.push('/');
                at.push_str(part);
                if let Some(target) = self.links.get(&at) {
                    let base = if target.starts_with('/') {
                        target.clone()
                    } else {
                        format!("{}/{target}", parent(&at))
                    };
                    next = Some(tidy(&format!("{base}/{}", parts[index + 1..].join("/"))));
                    break;
                }
            }
            match next {
                Some(resolved) => path = resolved,
                None => return path,
            }
        }
        path
    }

    /// True when `uid` may make `path`: root, or the nearest folder there
    /// is its own.
    fn writable(&self, path: &str, uid: u32) -> bool {
        if uid == 0 {
            return true;
        }
        let mut at = self.resolve(path);
        loop {
            if let Some(dir) = self.dirs.get(&at) {
                return dir.uid == uid;
            }
            if self.files.contains_key(&at) || at == "/" {
                return false;
            }
            at = parent(&at).to_owned();
        }
    }

    /// `mkdir -p`, failing as it would without permission.
    fn mkdir_p(&mut self, path: &str, uid: u32) -> Result<(), ExecOutput> {
        let path = self.resolve(path);
        if self.dirs.contains_key(&path) {
            return Ok(());
        }
        if !self.writable(&path, uid) {
            return Err(exit(
                1,
                format!("mkdir: cannot create directory '{path}': Permission denied"),
            ));
        }
        let mut at = String::new();
        for part in path.split('/').filter(|part| !part.is_empty()) {
            at.push('/');
            at.push_str(part);
            if self.files.contains_key(&at) {
                return Err(exit(1, format!("mkdir: '{at}': File exists")));
            }
            self.dirs
                .entry(at.clone())
                .or_insert_with(|| dir_of(0o755, uid));
        }
        Ok(())
    }

    /// `rm -rf`.
    /// Where `path` is with every part but the last followed, as `rm` and
    /// `test -L` see it.
    fn unfollowed(&self, path: &str) -> String {
        let path = tidy(path);
        let name = path.rsplit('/').next().unwrap_or_default();
        tidy(&format!("{}/{name}", self.resolve(parent(&path))))
    }

    /// Whether anything, a link included, is at `at`.
    fn there(&self, at: &str) -> bool {
        self.files.contains_key(at) || self.dirs.contains_key(at) || self.links.contains_key(at)
    }

    /// `rm -rf` as `uid`. Someone other than root removes only from a folder
    /// of its own, and a path it can't remove fails the command after the
    /// rest go.
    fn rm_rf(&mut self, paths: &[&str], uid: u32) -> ExecOutput {
        let mut denied = String::new();
        for path in paths.iter().filter(|path| **path != "--") {
            let at = self.unfollowed(path);
            if !self.there(&at) {
                continue;
            }
            if !self.writable(parent(&at), uid) {
                denied.push_str(&format!("rm: cannot remove '{path}': Permission denied\n"));
                continue;
            }
            self.remove_tree(&at);
        }
        match denied.is_empty() {
            true => out(""),
            false => exit(1, denied),
        }
    }

    /// What [`LEFT`] prints: each path that's there, a link included.
    fn left(&self, paths: &[&str]) -> ExecOutput {
        let mut found = String::new();
        for path in paths {
            if self.there(&self.unfollowed(path)) || self.there(&self.resolve(path)) {
                found.push_str(&format!("{path}\n"));
            }
        }
        out(found)
    }

    fn remove_tree(&mut self, path: &str) {
        let path = tidy(path);
        self.files.retain(|at, _| !within(at, &path));
        self.dirs.retain(|at, _| !within(at, &path));
        self.links.retain(|at, _| !within(at, &path));
    }

    /// `mv` of a folder and everything in it.
    fn move_tree(&mut self, from: &str, to: &str) {
        let moved = |at: &str| format!("{to}{}", &at[from.len()..]);
        let files: Vec<_> = self
            .files
            .keys()
            .filter(|at| within(at, from))
            .cloned()
            .collect();
        for at in files {
            let file = self.files.remove(&at).unwrap();
            self.files.insert(moved(&at), file);
        }
        let dirs: Vec<_> = self
            .dirs
            .keys()
            .filter(|at| within(at, from))
            .cloned()
            .collect();
        for at in dirs {
            let dir = self.dirs.remove(&at).unwrap();
            self.dirs.insert(moved(&at), dir);
        }
        let links: Vec<_> = self
            .links
            .keys()
            .filter(|at| within(at, from))
            .cloned()
            .collect();
        for at in links {
            let link = self.links.remove(&at).unwrap();
            self.links.insert(moved(&at), link);
        }
    }

    /// The names directly inside `dir`.
    fn children(&self, dir: &str) -> BTreeSet<String> {
        self.files
            .keys()
            .chain(self.dirs.keys())
            .chain(self.links.keys())
            .filter(|at| parent(at) == dir && at.as_str() != dir)
            .map(|at| file_name(at).to_owned())
            .collect()
    }

    fn put_text(&mut self, path: &str, text: &str, mode: u32, uid: u32) {
        self.files.insert(
            path.to_owned(),
            File {
                bytes: text.as_bytes().to_vec(),
                mode,
                uid,
                gid: uid,
            },
        );
    }

    fn download(&mut self, url: &str, path: &str, user: ExecUser) -> Result<Fetched, ExecError> {
        self.calls.push(Call::Download {
            url: url.to_owned(),
            path: path.to_owned(),
            user,
        });
        let Some(download) = self.downloads.get(url).cloned() else {
            return Err(ExecError::Transport(
                "the download failed: the server said 404 Not Found".to_owned(),
            ));
        };
        let path = self.resolve(path);
        if !self.dirs.contains_key(parent(&path)) || !self.writable(&path, user.uid()) {
            return Err(ExecError::Transport(format!("can't write {path}")));
        }
        self.put_text(&path, &download.body, 0o644, user.uid());
        Ok(Fetched {
            sha256: download.sha256,
            size: download.body.len() as u64,
        })
    }

    fn sh(
        &mut self,
        args: &[&str],
        user: ExecUser,
        env: &BTreeMap<&str, &str>,
    ) -> Result<ExecOutput, ExecError> {
        let ["-c", script, _, args @ ..] = args else {
            return Err(ExecError::Transport("the fake runs only sh -c".to_owned()));
        };
        let uid = user.uid();
        let done = |result: Result<ExecOutput, ExecOutput>| Ok(result.unwrap_or_else(|err| err));
        match (*script, args) {
            (LOCK, [dir, token]) => done(self.take_lock(dir, token, uid)),
            (UNLOCK, [dir, token]) => {
                let path = format!("{dir}/.lock");
                if self
                    .files
                    .get(&path)
                    .map(|file| file.text().trim().to_owned())
                    == Some((*token).to_owned())
                {
                    self.files.remove(&path);
                }
                Ok(out(""))
            }
            (STAGE, [dir, staging]) => done(self.stage(dir, staging, uid)),
            (UNPACK, [staging, target, names @ ..]) => {
                done(self.unpack(staging, target, names, uid))
            }
            (CHECK, [dir]) => Ok(self.check(dir)),
            (PUBLISH, [dir, staging, version, bin, replace, record, pairs @ ..]) => {
                done(self.publish(
                    dir,
                    staging,
                    version,
                    bin,
                    *replace == "1",
                    record,
                    pairs,
                    uid,
                ))
            }
            (RUN, [path, ..]) => {
                if !self.packages.contains("curl") {
                    return Ok(exit(127, "bash: line 1: curl: command not found"));
                }
                match self.files.get(*path) {
                    Some(file) if file.text() == CLAUDE_SCRIPT => {
                        Ok(self.install_claude(user, env))
                    }
                    _ => Err(ExecError::Transport(
                        "the fake doesn't know this install script".to_owned(),
                    )),
                }
            }
            (FINISH, [dir, version, link]) => {
                for name in self.children(dir) {
                    if name.starts_with(".staging-") {
                        self.remove_tree(&format!("{dir}/{name}"));
                    }
                }
                self.put_text(
                    &format!("{dir}/script"),
                    &format!("{version}\n"),
                    0o644,
                    uid,
                );
                self.put_text(&format!("{dir}/links"), &format!("{link}\n"), 0o644, uid);
                Ok(out(""))
            }
            (REMOVE, [dir]) => Ok(self.remove_tool(dir)),
            (LEFT, paths) => Ok(self.left(paths)),
            _ => Err(ExecError::Transport(
                "the fake doesn't know this script".to_owned(),
            )),
        }
    }

    fn take_lock(&mut self, dir: &str, token: &str, uid: u32) -> Result<ExecOutput, ExecOutput> {
        self.mkdir_p(dir, uid)?;
        let path = format!("{dir}/.lock");
        if let Some(file) = self.files.get(&path) {
            return Ok(ExecOutput {
                status: 3,
                stdout: file.bytes.clone(),
                stderr: Vec::new(),
            });
        }
        self.put_text(&path, &format!("{token}\n"), 0o644, uid);
        Ok(out(""))
    }

    fn stage(&mut self, dir: &str, staging: &str, uid: u32) -> Result<ExecOutput, ExecOutput> {
        self.mkdir_p(dir, uid)?;
        for name in self.children(dir) {
            if name.starts_with(".staging-") {
                self.remove_tree(&format!("{dir}/{name}"));
            }
        }
        self.mkdir_p(&format!("{dir}/{staging}/dl"), uid)?;
        self.mkdir_p(&format!("{dir}/{staging}/files"), uid)?;
        Ok(out(""))
    }

    fn unpack(
        &mut self,
        staging: &str,
        target: &str,
        names: &[&str],
        uid: u32,
    ) -> Result<ExecOutput, ExecOutput> {
        for name in names {
            let path = format!("{staging}/dl/{name}");
            let text = self.files.get(&path).map(File::text).unwrap_or_default();
            let Some(listing) = text.strip_prefix(&format!("{ARCHIVE}\n")) else {
                return Err(exit(2, "tar: This does not look like a tar archive"));
            };
            let mut entries: Vec<(String, String)> = listing
                .lines()
                .filter_map(|line| line.split_once('\t'))
                .map(|(path, text)| (path.to_owned(), text.to_owned()))
                .collect();
            let tops: BTreeSet<&str> = entries
                .iter()
                .map(|(path, _)| path.split('/').next().unwrap_or(""))
                .collect();
            if tops.len() == 1 && entries.iter().all(|(path, _)| path.contains('/')) {
                for (path, _) in &mut entries {
                    *path = path
                        .split_once('/')
                        .map(|(_, rest)| rest.to_owned())
                        .unwrap();
                }
            }
            for (rel, text) in entries {
                let (first, rest) = match rel.split_once('/') {
                    Some((first, rest)) => (first.to_owned(), format!("/{rest}")),
                    None => (rel.clone(), String::new()),
                };
                let first = first
                    .strip_suffix(&format!("-{target}"))
                    .unwrap_or(&first)
                    .to_owned();
                let at = format!("{staging}/files/{first}{rest}");
                self.mkdir_p(parent(&at), uid)?;
                self.put_text(&at, &format!("{text}\n"), 0o755, uid);
            }
            self.files.remove(&path);
        }
        let root = format!("{staging}/files/");
        let programs: Vec<String> = self
            .files
            .iter()
            .filter_map(|(at, file)| {
                let rel = at.strip_prefix(&root)?;
                (rel.matches('/').count() <= 1 && file.mode & 0o100 != 0)
                    .then(|| format!("./{rel}\n"))
            })
            .collect();
        Ok(out(programs.concat()))
    }

    fn check(&self, dir: &str) -> ExecOutput {
        let dir = self.resolve(dir);
        let Some(record) = self.files.get(&format!("{dir}/.install-complete")) else {
            return exit(1, "");
        };
        for line in record.text().lines() {
            if let Some(entry) = line.strip_prefix("entry ") {
                let path = self.resolve(&format!("{dir}/{entry}"));
                if self
                    .files
                    .get(&path)
                    .is_none_or(|file| file.mode & 0o111 == 0)
                {
                    return exit(1, "");
                }
            }
        }
        out("")
    }

    #[allow(clippy::too_many_arguments)]
    fn publish(
        &mut self,
        dir: &str,
        staging: &str,
        version: &str,
        bin: &str,
        replace: bool,
        record: &str,
        pairs: &[&str],
        uid: u32,
    ) -> Result<ExecOutput, ExecOutput> {
        let staged = format!("{dir}/{staging}");
        let files = format!("{staged}/files");
        if self.dirs.contains_key(&files) {
            self.put_text(
                &format!("{files}/.install-complete"),
                &format!("{record}\n"),
                0o644,
                uid,
            );
            self.remove_tree(&format!("{dir}/{version}"));
            self.move_tree(&files, &format!("{dir}/{version}"));
        }
        self.remove_tree(&staged);
        let current = format!("{dir}/current");
        let prev = self.links.get(&current).cloned();
        self.links.insert(current, version.to_owned());
        self.mkdir_p(bin, uid)?;
        let mut said = String::new();
        let mut made = String::new();
        for pair in pairs.chunks(2) {
            let [name, entry] = pair else { break };
            let link = format!("{bin}/{name}");
            let want = format!("{dir}/current/{entry}");
            let cur = self.links.get(&link);
            let there = self.files.contains_key(&link) || self.dirs.contains_key(&link);
            if (!there && cur.is_none()) || cur == Some(&want) || replace {
                self.files.remove(&link);
                self.links.insert(link.clone(), want);
                made.push_str(&format!("{link}\n"));
                said.push_str(&format!("linked {link}\n"));
            } else {
                said.push_str(&format!("kept {link}\n"));
            }
        }
        self.put_text(&format!("{dir}/links"), &made, 0o644, uid);
        for name in self.children(dir) {
            let keep = name.starts_with('.')
                || name == version
                || Some(&name) == prev.as_ref()
                || matches!(name.as_str(), "current" | "links" | "script");
            if !keep {
                self.remove_tree(&format!("{dir}/{name}"));
            }
        }
        Ok(out(said))
    }

    fn remove_tool(&mut self, dir: &str) -> ExecOutput {
        let script = self.files.contains_key(&format!("{dir}/script"));
        if !self.links.contains_key(&format!("{dir}/current")) && !script {
            self.files.remove(&format!("{dir}/.lock"));
            if self.children(dir).is_empty() {
                self.dirs.remove(dir);
            }
            return exit(4, "");
        }
        let links = self
            .files
            .get(&format!("{dir}/links"))
            .map(File::text)
            .unwrap_or_default();
        for link in links.lines() {
            if script {
                self.files.remove(link);
                self.links.remove(link);
            } else if self
                .links
                .get(link)
                .is_some_and(|target| within(target, dir))
            {
                self.links.remove(link);
            }
        }
        self.remove_tree(dir);
        out("")
    }
}
