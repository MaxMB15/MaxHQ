// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Whether a folder's own files may run when an agent starts there. A
//! cloned repository's hooks and MCP servers run as you at the CLI's start,
//! and its settings can send the login to another URL. So until you trust
//! what's there, each CLI starts without any of it: Claude with
//! [`RESTRICTED_ARGS`], Codex with its thread untrusted, and Cursor with a
//! data folder of MaxHQ's own for each agent, [`cursor_data_dir`]. That's
//! where Cursor keeps the MCP servers you approved for a project, so none of
//! them start, and one an agent approves there reaches no other agent. Its
//! sign-in and settings are elsewhere, so they stay. Cursor has no switch
//! for a checkout's own `.cursor/hooks.json`: 2026.10.22 doesn't run it over
//! ACP, but a version that does would run it restricted or not.
//!
//! A hook that calls `scripts/x.sh` runs that script, so the commands in a
//! hook file are followed to the folder's files they run, and those count
//! too. A command MaxHQ can't follow, such as `npm run lint`, counts by its
//! text, and its card says a change to what it runs won't ask again. A
//! script's own calls aren't followed.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::io::{self, Read};
use std::iter::Peekable;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::str::Chars;
use std::time::Duration;

use maxhq_protocol::attention::{AttentionEvent, AttentionKind};
use maxhq_protocol::guest::{FolderScan, ScannedFile, ShownFile};
use maxhq_protocol::ids::{AgentId, AttentionId, ClientId, WorkspaceId};
use maxhq_protocol::model::Vendor;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    ItemBody, ItemId, ItemStatus, Question, QuestionOption, TimelineItem,
};
use ring::digest::{Context, SHA256};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::Db;
use crate::drive::codex::requests::visible;

/// What keeps a CLI from a folder's settings, MCP servers and instructions.
pub const RESTRICTED_ARGS: [&str; 3] = ["--setting-sources", "user", "--strict-mcp-config"];

/// The answer that trusts the folder.
pub const TRUST_LABEL: &str = "Trust this folder";

/// The answer that leaves the agent as it is.
pub const RESTRICTED_LABEL: &str = "Keep restricted";

/// The one answer to a Cursor card from before Cursor could start without
/// a folder's files, which still shows after an update. It stores the files
/// the way a trusted answer does, so the card doesn't show again until they
/// change.
pub const NOTED_LABEL: &str = "Got it";

/// The variable that moves Cursor's data folder.
pub const CURSOR_DATA_VAR: &str = "CURSOR_DATA_DIR";

/// The card's one question.
pub const QUESTION: &str = "folder-trust";

/// What one CLI reads from a folder of its own.
struct Reads {
    vendor: Vendor,
    /// The folder, which a link can lead anywhere.
    dir: &'static str,
    /// Files read through a link, since the CLI reads them that way.
    files: &'static [&'static str],
    /// Folders walked whole. The rest stays unread, since
    /// `.claude/worktrees` can hold hundreds of gigabytes.
    walked: &'static [&'static str],
    /// Those of `files` whose commands are followed to what they run.
    hooks: &'static [&'static str],
}

/// What each CLI reads that can run something or widen what an agent may
/// do. Instructions alone don't count: `AGENTS.md`, Codex's skills, which
/// it loads either way, and Cursor's rules, skills, agents and commands.
/// Nor does `.cursor/cli.json`, which `--disable-project-configs` keeps
/// out.
const READS: [Reads; 3] = [
    Reads {
        vendor: Vendor::Claude,
        dir: ".claude",
        files: &[".claude/settings.json", ".claude/settings.local.json"],
        walked: &[
            ".claude/hooks",
            ".claude/agents",
            ".claude/skills",
            ".claude/commands",
        ],
        hooks: &[".claude/settings.json", ".claude/settings.local.json"],
    },
    Reads {
        vendor: Vendor::Codex,
        dir: ".codex",
        files: &[".codex/config.toml", ".codex/hooks.json"],
        walked: &[".codex/rules"],
        hooks: &[".codex/hooks.json"],
    },
    Reads {
        vendor: Vendor::Cursor,
        dir: ".cursor",
        files: &[
            ".cursor/mcp.json",
            ".cursor/hooks.json",
            ".cursor/sandbox.json",
        ],
        walked: &[".cursor/hooks"],
        hooks: &[".cursor/hooks.json"],
    },
];

/// Claude's MCP file, which it reads from every folder up to the root.
const MCP_FILE: &str = ".mcp.json";

/// How many entries are hashed by content.
const MAX_ENTRIES: usize = 2000;

/// How many bytes are hashed in all.
const MAX_BYTES: u64 = 64 * 1024 * 1024;

/// How many entries are looked at before the scan gives up.
const GIVE_UP: usize = 100_000;

/// How much of a file a card shows.
const SHOWN_BYTES: usize = 32 * 1024;

/// The most a guest's scan may come to, under the link's limit for a message.
const GUEST_REPLY: usize = 3 * 1024 * 1024;

/// How long an agent's start waits for a guest's scan.
const GUEST_WAIT: Duration = Duration::from_secs(10);

/// How many file names a card lists.
const LISTED: usize = 200;

/// How much of `.git` and its friends is read.
const GIT_FILE_BYTES: u64 = 4096;

/// The name for everything past the limits, hashed by name and size.
const PAST_LIMIT: &str = "(past the limit)";

/// The name for an MCP file in a folder above the checkout.
const ABOVE: &str = "(above)/.mcp.json";

/// The digest of a file that couldn't be read.
const UNREADABLE: &str = "unreadable";

/// The digest of a file a hook runs that isn't there yet, so adding it
/// changes the fingerprint.
const MISSING: &str = "missing";

/// What a hook file's entry is named for a command MaxHQ can't follow,
/// after the file's own name.
const UNFOLLOWED: &str = "runs a command MaxHQ can't follow";

/// Programs that run the file named after them and, without a flag, nothing
/// else of the folder's.
const INTERPRETERS: [&str; 9] = [
    "sh", "bash", "zsh", "dash", "python", "python3", "node", "ruby", "perl",
];

/// The variable Claude sets to the folder it started in, which hooks use to
/// name their scripts.
const PROJECT_VAR: &str = "CLAUDE_PROJECT_DIR";

/// The keys in a hook file whose value the CLI runs: a hook's or the status
/// line's command, and Claude's helpers for logins and headers.
const COMMAND_KEYS: [&str; 5] = [
    "command",
    "apiKeyHelper",
    "awsAuthRefresh",
    "awsCredentialExport",
    "otelHeadersHelper",
];

/// Looks at what in `cwd` can run when an agent starts there. `home`
/// holds your own settings, so its `.claude` isn't the folder's. `locked`
/// names the skills in the project's lock. `inside` keeps a link from being
/// read anywhere else.
pub fn scan(
    cwd: &Path,
    home: Option<&Path>,
    locked: &[String],
    inside: Option<&Path>,
) -> FolderScan {
    let cwd = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_owned());
    let found = checkout_of(&cwd);
    let key = found
        .as_ref()
        .map_or_else(|| cwd.display().to_string(), |(_, key)| key.clone());
    if inside.is_some_and(|inside| !cwd.starts_with(inside)) {
        return FolderScan {
            key,
            failed: true,
            ..FolderScan::default()
        };
    }
    let mut walk = Walk {
        files: Vec::new(),
        shown: Vec::new(),
        failed: false,
        seen: 0,
        bytes: 0,
        past: 0,
        rest: Context::new(&SHA256),
        inside,
        root: found
            .as_ref()
            .map_or_else(|| cwd.clone(), |(root, _)| root.clone()),
        cwd: cwd.clone(),
    };
    // Inside a checkout the CLI reads each level from the folder up to its
    // root. Outside one nothing marks an end, so that's every level up to
    // your home. Above those it still reads MCP files.
    for (up, level) in cwd.ancestors().enumerate() {
        let prefix = match &found {
            Some((root, _)) => level
                .strip_prefix(root)
                .ok()
                .map(|from_root| from_root.display().to_string()),
            None if home.is_some_and(|home| home.starts_with(level) && home != level) => None,
            None => Some("../".repeat(up).trim_end_matches('/').to_owned()),
        };
        match prefix {
            Some(prefix) => walk.level(level, &prefix, home, locked),
            None => walk.above(level),
        }
    }
    walk.finish(key)
}

/// The checkout `cwd` is in, as its root and project key. Git never runs
/// here, since the folder isn't trusted, and `.git/config` can claim any
/// origin.
pub(crate) fn checkout_of(cwd: &Path) -> Option<(PathBuf, String)> {
    for level in cwd.ancestors() {
        let dot_git = level.join(".git");
        let Ok(meta) = dot_git.symlink_metadata() else {
            continue;
        };
        let common = if meta.is_dir() {
            fs::canonicalize(&dot_git).ok()
        } else if meta.is_file() {
            worktree_common(&dot_git)
        } else {
            None
        };
        let key = common.unwrap_or_else(|| level.to_owned());
        return Some((level.to_owned(), key.display().to_string()));
    }
    None
}

/// The common git folder of a linked worktree whose `.git` file is at
/// `dot_git`. The folder it names has to name this worktree back, or a
/// hostile `.git` file could claim another project's key.
fn worktree_common(dot_git: &Path) -> Option<PathBuf> {
    let text = read_small(dot_git)?;
    let named = Path::new(text.strip_prefix("gitdir:")?.trim());
    let git_dir = fs::canonicalize(dot_git.parent()?.join(named)).ok()?;
    let back = read_small(&git_dir.join("gitdir"))?;
    let back = fs::canonicalize(git_dir.join(back.trim())).ok()?;
    if back != fs::canonicalize(dot_git).ok()? {
        return None;
    }
    let common = read_small(&git_dir.join("commondir"))?;
    fs::canonicalize(git_dir.join(common.trim())).ok()
}

/// A small text file, without waiting on anything that isn't a file.
fn read_small(path: &Path) -> Option<String> {
    let file = open_regular(path, true).ok()?;
    let mut text = String::new();
    file.take(GIT_FILE_BYTES).read_to_string(&mut text).ok()?;
    Some(text)
}

/// Opens a regular file. A pipe or device is an error rather than a read
/// that never ends.
fn open_regular(path: &Path, follow: bool) -> io::Result<File> {
    let mut flags = libc::O_NONBLOCK;
    if !follow {
        flags |= libc::O_NOFOLLOW;
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open(path)?;
    if file.metadata()?.is_file() {
        Ok(file)
    } else {
        Err(io::Error::other("not a regular file"))
    }
}

/// Whether a read failed only because nothing is there.
fn absent(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR))
}

/// Bytes as hex.
fn hex(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A file's digest and the start of its text.
struct Content {
    digest: String,
    text: String,
    cut: bool,
}

/// One scan's running state.
struct Walk<'a> {
    files: Vec<ScannedFile>,
    shown: Vec<ShownFile>,
    failed: bool,
    /// Entries hashed.
    seen: usize,
    /// Bytes hashed.
    bytes: u64,
    /// Entries past the limits.
    past: usize,
    /// Their names and sizes.
    rest: Context,
    inside: Option<&'a Path>,
    /// The checkout's root, or the folder outside one. A file a hook runs
    /// counts only under it, since nothing else is the folder's.
    root: PathBuf,
    /// The folder, which hooks run in.
    cwd: PathBuf,
}

impl Walk<'_> {
    /// Everything at one level from a checkout's root to the folder.
    fn level(&mut self, level: &Path, prefix: &str, home: Option<&Path>, locked: &[String]) {
        let named = |path: &str| match prefix {
            "" => path.to_owned(),
            prefix => format!("{prefix}/{path}"),
        };
        self.read_through(&level.join(MCP_FILE), named(MCP_FILE), named(MCP_FILE));
        // Your own settings run however the CLI is started.
        if home.is_some_and(|home| home == level) {
            return;
        }
        for reads in &READS {
            let dir = level.join(reads.dir);
            if dir.symlink_metadata().is_ok_and(|meta| meta.is_symlink()) {
                self.link(&dir, named(reads.dir));
                // The paths below would all lead wherever this does.
                let out = self.inside.is_some_and(|inside| {
                    fs::canonicalize(&dir).map_or(true, |real| !real.starts_with(inside))
                });
                if out {
                    self.failed = true;
                    continue;
                }
            }
            let mut hooks = Vec::new();
            for path in reads.files {
                let (shown, past) = (self.shown.len(), self.past);
                self.read_through(&level.join(path), named(path), named(path));
                if !reads.hooks.contains(path) {
                    continue;
                }
                match self.shown.get(shown) {
                    Some(read) => hooks.push((named(path), read.text.clone(), read.cut)),
                    // Past the limits, so its commands can't be read.
                    None if self.past > past => self.unfollowed(&named(path), ""),
                    None => {}
                }
            }
            for walked in reads.walked {
                // Delivered links are only ever at the root.
                let locked = if prefix.is_empty() { locked } else { &[] };
                self.tree(&level.join(walked), &named(walked), locked);
            }
            // After the folders, so a script in one isn't counted twice.
            for (hook, text, cut) in hooks {
                self.follow_hooks(level, &hook, &text, cut);
            }
        }
    }

    /// Counts what the commands in a hook file run from the folder. `hook`
    /// is the file's name, at `level`, and `text` what was read of it.
    fn follow_hooks(&mut self, level: &Path, hook: &str, text: &str, cut: bool) {
        let project = self.cwd.display().to_string();
        let commands = (!cut).then(|| hook_commands(text, &project)).flatten();
        let Some(commands) = commands else {
            // The CLI may read what MaxHQ can't, so all of it counts as one.
            return self.unfollowed(hook, text);
        };
        for (line, words) in commands {
            let followed = words.is_some_and(|words| self.follow(level, hook, &words));
            if !followed {
                self.unfollowed(hook, &line);
            }
        }
    }

    /// Counts a command MaxHQ can't follow by its text, once.
    fn unfollowed(&mut self, hook: &str, line: &str) {
        let file = ScannedFile {
            name: format!("{hook} {UNFOLLOWED}"),
            digest: hex(ring::digest::digest(&SHA256, line.as_bytes())),
        };
        if !self.files.contains(&file) {
            self.files.push(file);
        }
    }

    /// Counts the folder's files one command runs, and returns whether it
    /// could be followed: a program or script of the folder's, or an
    /// interpreter running one with no flag before it. Anything else, such
    /// as `npx` or `make`, may run any of the folder's files.
    fn follow(&mut self, level: &Path, hook: &str, words: &[String]) -> bool {
        let mut words = without_assignments(words);
        loop {
            let Some((program, rest)) = words.split_first() else {
                return true;
            };
            if program.contains('/') && self.runs(level, hook, program, true) == Some(true) {
                for word in rest.iter().filter(|word| !word.starts_with('-')) {
                    self.runs(level, hook, word, false);
                }
                return true;
            }
            let name = program.rsplit('/').next().unwrap_or(program);
            if name == "env" {
                words = without_assignments(rest);
                if words.first().is_some_and(|word| word.starts_with('-')) {
                    return false;
                }
                continue;
            }
            // A program of someone else's, such as `/bin/echo`, isn't the
            // folder's, but what it reads may be.
            if !INTERPRETERS.contains(&name) {
                return false;
            }
            // With no script it reads the hook's input, and a flag may hand
            // it code of its own.
            let Some((script, args)) = rest.split_first() else {
                return false;
            };
            if script.starts_with('-') || self.runs(level, hook, script, true).is_none() {
                return false;
            }
            for word in args.iter().filter(|word| !word.starts_with('-')) {
                self.runs(level, hook, word, false);
            }
            return true;
        }
    }

    /// Counts the file `word` names, from the folder or from `level`, as
    /// `<hook> runs <path from the root>`. `must` says it's what runs, so
    /// one that isn't there yet counts as missing, and a folder where it
    /// should be can't be followed. Returns whether it's the folder's, or
    /// `None` when it can't be followed.
    fn runs(&mut self, level: &Path, hook: &str, word: &str, must: bool) -> Option<bool> {
        let mut bases = vec![self.cwd.clone()];
        if level != self.cwd {
            bases.push(level.to_owned());
        }
        let mut first = None;
        let mut found = false;
        for base in bases {
            // Named by where it sits, but read the way the shell finds it,
            // through any link on the way.
            let path = base.join(word);
            let plain = lexical(&path);
            let Ok(from_root) = plain.strip_prefix(&self.root) else {
                continue;
            };
            let from_root = from_root.display().to_string();
            first.get_or_insert_with(|| from_root.clone());
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                if must {
                    return None;
                }
                continue;
            }
            found = true;
            let name = format!("{hook} runs {from_root}");
            if !self.counted(&name, &from_root, &path, &plain, hook) {
                self.read_through(&path, name.clone(), name);
            }
        }
        let Some(from_root) = first else {
            return Some(false);
        };
        let name = format!("{hook} runs {from_root}");
        if must && !found && !self.files.iter().any(|file| file.name == name) {
            self.files.push(ScannedFile {
                name,
                digest: MISSING.to_owned(),
            });
        }
        Some(true)
    }

    /// Whether a file a hook runs is already counted for every CLI that
    /// reads the hook: by this name, or as itself in a folder that's walked,
    /// when no link is on its way.
    fn counted(&self, name: &str, from_root: &str, path: &Path, plain: &Path, hook: &str) -> bool {
        let walked = || {
            fs::canonicalize(path).is_ok_and(|real| real == plain)
                && READS
                    .iter()
                    .all(|reads| !read_by(hook, reads.vendor) || read_by(from_root, reads.vendor))
        };
        self.files.iter().any(|file| file.name == name)
            || (self.files.iter().any(|file| file.name == from_root) && walked())
    }

    /// The MCP file of a folder above the checkout. How deep a worktree sits
    /// mustn't change the result, so these go by content, and one that
    /// matches the checkout's own adds nothing.
    fn above(&mut self, level: &Path) {
        let before = self.files.len();
        let shown_before = self.shown.len();
        let path = level.join(MCP_FILE);
        self.read_through(&path, ABOVE.to_owned(), path.display().to_string());
        let Some(new) = self.files.get(before) else {
            return;
        };
        let known = self.files[..before].iter().any(|file| {
            file.digest == new.digest && matches!(file.name.as_str(), MCP_FILE | ABOVE)
        });
        if known {
            self.files.truncate(before);
            self.shown.truncate(shown_before);
        }
    }

    /// Hashes and shows a file the CLI reads through a link.
    fn read_through(&mut self, path: &Path, name: String, shown: String) {
        match fs::canonicalize(path) {
            // A file that's where it says is fine, even above the work folder.
            Ok(real)
                if real == path || self.inside.is_none_or(|inside| real.starts_with(inside)) => {}
            // A link leading out of the work folder stays unread, since a
            // guest reads as root.
            Ok(_) => {
                self.failed = true;
                self.link(path, name);
                return;
            }
            Err(err) if absent(&err) => return,
            Err(_) => return self.unreadable(name),
        }
        match self.content(path, true) {
            Ok(Some(content)) => {
                self.files.push(ScannedFile {
                    name,
                    digest: content.digest,
                });
                self.shown.push(ShownFile {
                    name: shown,
                    text: content.text,
                    cut: content.cut,
                });
            }
            Ok(None) => self.note_past(&name, path),
            Err(err) if absent(&err) => {}
            Err(_) => self.unreadable(name),
        }
    }

    /// Counts a file that's there but couldn't be read.
    fn unreadable(&mut self, name: String) {
        self.failed = true;
        self.files.push(ScannedFile {
            name,
            digest: UNREADABLE.to_owned(),
        });
    }

    /// Reads a file's digest, and the start of its text. `None` means the
    /// limits were reached first.
    fn content(&mut self, path: &Path, follow: bool) -> io::Result<Option<Content>> {
        let mut file = open_regular(path, follow)?;
        let size = file.metadata()?.len();
        if self.seen >= MAX_ENTRIES || self.bytes.saturating_add(size) > MAX_BYTES {
            return Ok(None);
        }
        self.seen += 1;
        let mut hash = Context::new(&SHA256);
        let mut head = Vec::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut cut = false;
        loop {
            let got = file.read(&mut buffer)?;
            // A file can grow after its size was read.
            let left = usize::try_from(MAX_BYTES - self.bytes).unwrap_or(usize::MAX);
            let taken = got.min(left);
            if taken == 0 {
                cut |= got > 0;
                break;
            }
            hash.update(&buffer[..taken]);
            self.bytes += taken as u64;
            let room = SHOWN_BYTES - head.len();
            head.extend_from_slice(&buffer[..taken.min(room)]);
            cut |= taken > room;
        }
        Ok(Some(Content {
            digest: hex(hash.finish()),
            text: String::from_utf8_lossy(&head).into_owned(),
            cut,
        }))
    }

    /// Adds a link by its target's text.
    fn link(&mut self, path: &Path, name: String) {
        match fs::read_link(path) {
            Ok(target) => {
                let digest = ring::digest::digest(&SHA256, target.as_os_str().as_bytes());
                self.files.push(ScannedFile {
                    name,
                    digest: hex(digest),
                });
            }
            Err(_) => self.unreadable(name),
        }
    }

    /// Adds an entry past the limits by its name and size.
    fn note_past(&mut self, name: &str, path: &Path) {
        let size = path.symlink_metadata().map_or(0, |meta| meta.len());
        self.rest.update(name.as_bytes());
        self.rest.update(&[0]);
        self.rest.update(&size.to_be_bytes());
        self.past += 1;
    }

    /// Walks a folder without following a link.
    fn tree(&mut self, dir: &Path, name: &str, locked: &[String]) {
        let mut stack = vec![(dir.to_owned(), name.to_owned())];
        while let Some((path, name)) = stack.pop() {
            if self.seen + self.past >= GIVE_UP {
                self.failed = true;
                return;
            }
            let meta = match path.symlink_metadata() {
                Ok(meta) => meta,
                Err(err) if absent(&err) => continue,
                Err(_) => {
                    self.unreadable(name);
                    continue;
                }
            };
            if meta.is_symlink() {
                if delivered(&path, &name, locked) {
                    continue;
                }
                if self.seen >= MAX_ENTRIES {
                    self.note_past(&name, &path);
                } else {
                    self.seen += 1;
                    self.link(&path, name);
                }
            } else if meta.is_dir() {
                match sorted_entries(&path) {
                    // The stack pops the last, so the first goes on last.
                    Ok(entries) => {
                        for entry in entries.into_iter().rev() {
                            let child = format!("{name}/{}", entry.to_string_lossy());
                            stack.push((path.join(&entry), child));
                        }
                    }
                    Err(err) if absent(&err) => {}
                    Err(_) => self.unreadable(name),
                }
            } else {
                match self.content(&path, false) {
                    Ok(Some(content)) => self.files.push(ScannedFile {
                        name,
                        digest: content.digest,
                    }),
                    Ok(None) => self.note_past(&name, &path),
                    Err(err) if absent(&err) => {}
                    // A pipe or device in here counts against the folder.
                    Err(_) => self.unreadable(name),
                }
            }
        }
    }

    fn finish(mut self, key: String) -> FolderScan {
        if self.past > 0 {
            self.files.push(ScannedFile {
                name: PAST_LIMIT.to_owned(),
                digest: hex(self.rest.finish()),
            });
        }
        self.files
            .sort_by(|a, b| (&a.name, &a.digest).cmp(&(&b.name, &b.digest)));
        FolderScan {
            key,
            files: self.files,
            shown: self.shown,
            failed: self.failed,
        }
    }
}

/// A folder's entries, in an order every scan agrees on.
fn sorted_entries(dir: &Path) -> io::Result<Vec<OsString>> {
    let mut entries = fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<io::Result<Vec<_>>>()?;
    entries.sort();
    Ok(entries)
}

/// Whether a link is the one MaxHQ delivers for a skill in the project's
/// lock. A link to any other skill counts, or a hostile branch could commit
/// `.maxhq/skills/evil` and a link to it.
fn delivered(path: &Path, name: &str, locked: &[String]) -> bool {
    let Some(skill) = name.strip_prefix(".claude/skills/") else {
        return false;
    };
    locked.iter().any(|known| known == skill)
        && fs::read_link(path)
            .is_ok_and(|target| target == Path::new("../../.maxhq/skills").join(skill))
}

/// Every command in a hook file, under any of [`COMMAND_KEYS`]: its text,
/// and its words when MaxHQ can follow them. `project` stands for the
/// folder. `None` when the file isn't JSON.
fn hook_commands(text: &str, project: &str) -> Option<Vec<(String, Option<Vec<String>>)>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let mut found = Vec::new();
    let mut stack = vec![&value];
    while let Some(value) = stack.pop() {
        match value {
            Value::Object(map) => {
                for (key, value) in map {
                    if COMMAND_KEYS.contains(&key.as_str()) {
                        found.push((value.to_string(), command_words(value, project)));
                    } else {
                        stack.push(value);
                    }
                }
            }
            Value::Array(items) => stack.extend(items),
            _ => {}
        }
    }
    Some(found)
}

/// A command's words: a line for the shell, or a list run as it is.
fn command_words(command: &Value, project: &str) -> Option<Vec<String>> {
    match command {
        Value::String(line) => shell_words(line, project),
        Value::Array(words) => words
            .iter()
            .map(|word| word.as_str().map(str::to_owned))
            .collect(),
        _ => None,
    }
}

/// A shell line's words, as `sh` splits a plain one, with
/// `$CLAUDE_PROJECT_DIR` as `project`. Anything else the shell acts on,
/// such as a pipe, another variable, a glob or `~`, comes to `None`.
fn shell_words(line: &str, project: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => words.extend(word.take()),
            '\'' => {
                let word = word.get_or_insert_with(String::new);
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => word.push(c),
                    }
                }
            }
            '"' => {
                let word = word.get_or_insert_with(String::new);
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            c @ ('"' | '\\' | '$' | '`') => word.push(c),
                            c => {
                                word.push('\\');
                                word.push(c);
                            }
                        },
                        '$' => word.push_str(variable(&mut chars, project)?),
                        '`' => return None,
                        c => word.push(c),
                    }
                }
            }
            '\\' => match chars.next()? {
                '\n' => return None,
                c => word.get_or_insert_with(String::new).push(c),
            },
            // Unquoted, the shell splits it on spaces and expands globs.
            '$' => {
                let value = variable(&mut chars, project)?;
                if value.contains(|c: char| c.is_whitespace() || "*?[".contains(c)) {
                    return None;
                }
                word.get_or_insert_with(String::new).push_str(value);
            }
            c if c.is_alphanumeric() || "-_./:=@%+,".contains(c) => {
                word.get_or_insert_with(String::new).push(c);
            }
            _ => return None,
        }
    }
    words.extend(word);
    Some(words)
}

/// The value of the variable after a `$`. Only the folder's is known.
fn variable<'a>(chars: &mut Peekable<Chars<'_>>, project: &'a str) -> Option<&'a str> {
    let braced = chars.next_if_eq(&'{').is_some();
    let mut name = String::new();
    while let Some(c) = chars.next_if(|c| c.is_ascii_alphanumeric() || *c == '_') {
        name.push(c);
    }
    if braced && chars.next() != Some('}') {
        return None;
    }
    (name == PROJECT_VAR).then_some(project)
}

/// A command's words after the variables it sets for itself.
fn without_assignments(words: &[String]) -> &[String] {
    let set = words
        .iter()
        .take_while(|word| {
            word.split_once('=').is_some_and(|(name, _)| {
                name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            })
        })
        .count();
    &words[set..]
}

/// A path with its `.` and `..` worked out, without reading anything.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            part => out.push(part),
        }
    }
    out
}

/// The fingerprint of a scan's files: the same for the same paths and
/// content in any worktree of a project.
pub fn fingerprint(files: &[ScannedFile]) -> String {
    let mut hash = Context::new(&SHA256);
    for file in files {
        hash.update(&(file.name.len() as u64).to_be_bytes());
        hash.update(file.name.as_bytes());
        hash.update(&(file.digest.len() as u64).to_be_bytes());
        hash.update(file.digest.as_bytes());
    }
    hex(hash.finish())
}

/// The data folder `agent`, a Cursor, runs with until you trust its folder,
/// in its config folder `config`. Cursor keeps the MCP servers you approved
/// for a project in its data folder, so none are approved in this one. Each
/// agent has its own, since its shell can approve one there too.
pub fn cursor_data_dir(config: &Path, agent: AgentId) -> PathBuf {
    config.join("maxhq-restricted").join(agent.to_string())
}

/// What in a scan one CLI reads, under a key of its own, since trusting a
/// folder's files for one CLI says nothing about another's. Claude's keys
/// stay as they were before the others had any. Cursor's changed when it
/// could start without them: a `cursor:` pair only ever said you'd seen the
/// files, not that you trusted them.
pub fn only(scan: &FolderScan, vendor: Vendor) -> FolderScan {
    let key = match vendor {
        Vendor::Claude => scan.key.clone(),
        Vendor::Cursor => format!("cursor-trust:{}", scan.key),
        vendor => format!("{}:{}", vendor.name().to_lowercase(), scan.key),
    };
    FolderScan {
        key,
        files: scan
            .files
            .iter()
            .filter(|file| read_by(&file.name, vendor))
            .cloned()
            .collect(),
        shown: scan
            .shown
            .iter()
            .filter(|shown| read_by(&shown.name, vendor))
            .cloned()
            .collect(),
        // A read that failed may have been of any CLI's files.
        failed: scan.failed,
    }
}

/// Whether a CLI reads the file a scan named so. A name no CLI's folder
/// claims, such as the one for everything past the limits, counts for all.
fn read_by(name: &str, vendor: Vendor) -> bool {
    // What a hook runs counts for the hook's CLI, wherever it sits.
    if let Some((hook, _)) = name.split_once(" runs ") {
        return read_by(hook, vendor);
    }
    if name.rsplit('/').next() == Some(MCP_FILE) {
        return vendor == Vendor::Claude;
    }
    let mut readers = READS
        .iter()
        .filter(|reads| name.split('/').any(|part| part == reads.dir))
        .peekable();
    readers.peek().is_none() || readers.any(|reads| reads.vendor == vendor)
}

/// The folders a Codex thread in `cwd` may look its trust up for: each one
/// from `cwd` up, and the main folder of the checkout, which its worktrees
/// share. `key` is the scan's own, before [`only`]. `here` says `cwd` is on
/// this device, so its real path counts too.
pub fn codex_paths(cwd: &Path, key: &str, here: bool) -> Vec<String> {
    let mut paths = BTreeSet::new();
    let mut add = |path: &Path| {
        paths.extend(path.ancestors().map(|level| level.display().to_string()));
    };
    add(cwd);
    if here && let Ok(real) = fs::canonicalize(cwd) {
        add(&real);
    }
    // A guest's key starts with its workspace, which has no colon in it.
    let common = key
        .strip_prefix("guest:")
        .and_then(|rest| rest.split_once(':'))
        .map_or(key, |(_, key)| key);
    let common = Path::new(common);
    let main = match common.file_name() {
        Some(name) if name == ".git" => common.parent().unwrap_or(common),
        _ => common,
    };
    paths.insert(main.display().to_string());
    paths.into_iter().collect()
}

/// Scans a folder on this device.
pub fn scan_here(cwd: &Path, locked: &[String]) -> FolderScan {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    scan(cwd, home.as_deref(), locked, None)
}

/// Scans a folder for the host, from inside a guest, which reads as root.
pub fn scan_in_guest(cwd: &Path, home: &Path, locked: &[String], work: &Path) -> FolderScan {
    let work = fs::canonicalize(work).unwrap_or_else(|_| work.to_owned());
    fit(scan(cwd, Some(home), locked, Some(&work)), GUEST_REPLY)
}

/// A reply too long for the link would never arrive, so it says it failed.
fn fit(mut scan: FolderScan, most: usize) -> FolderScan {
    if serde_json::to_vec(&scan).map_or(true, |bytes| bytes.len() > most) {
        scan.failed = true;
        scan.files.clear();
        scan.shown.clear();
    }
    scan
}

/// What a guest's answer comes to on the host. A workspace keeps to its own
/// projects, so a guest can't name a pair another place trusts. No answer
/// counts as a failed read, and an agent doesn't wait long for one.
pub async fn from_guest(
    read: impl Future<Output = Result<FolderScan, String>>,
    workspace: WorkspaceId,
    cwd: &str,
) -> FolderScan {
    let answer = tokio::time::timeout(GUEST_WAIT, read)
        .await
        .unwrap_or_else(|_| Err("the guest took too long".to_owned()));
    let mut scan = answer.unwrap_or_else(|err| {
        tracing::warn!(%err, cwd, "couldn't read a folder in a container");
        FolderScan {
            key: cwd.to_owned(),
            failed: true,
            ..FolderScan::default()
        }
    });
    scan.key = format!("guest:{workspace}:{}", scan.key);
    scan
}

/// What MaxHQ's own install wrote stays trusted, for each CLI, if what was
/// there before was trusted or empty. A hostile file that was already there
/// is never blessed by an install landing next to it.
pub fn keep_trust(db: &Db, before: &FolderScan, after: &FolderScan) {
    for reads in &READS {
        keep_trust_for(db, &only(before, reads.vendor), &only(after, reads.vendor));
    }
}

fn keep_trust_for(db: &Db, before: &FolderScan, after: &FolderScan) {
    if before.failed || after.failed || after.files.is_empty() || before.files == after.files {
        return;
    }
    let was_fine = before.files.is_empty()
        || db
            .folder_trusted(&before.key, &fingerprint(&before.files))
            .unwrap_or(false);
    if !was_fine {
        return;
    }
    let by = ClientId::derive("maxhq-install");
    let at = Timestamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64),
    );
    if let Err(err) = db.trust_folder(&after.key, &fingerprint(&after.files), at, by, &after.files)
    {
        tracing::warn!(%err, "couldn't note what an install added to a trusted folder");
    }
}

/// The card asking whether to trust a folder, as it opens and as it shows
/// in the timeline.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Card {
    /// The attention event.
    pub event: AttentionEvent,
    /// The timeline's question.
    pub item: TimelineItem,
}

impl Card {
    /// Whether the card's question offers this answer.
    pub fn offers(&self, label: &str) -> bool {
        matches!(&self.item.body, ItemBody::Question { questions, .. }
        if questions.iter().any(|question| {
            question.options.iter().any(|option| option.label == label)
        }))
    }
}

/// A card's ID, so restarting its agent doesn't ask again.
pub fn card_id(agent: AgentId, fingerprint: &str) -> AttentionId {
    AttentionId::derive(&format!("folder-trust:{agent}:{fingerprint}"))
}

/// The card for an agent of `vendor` in `cwd`, from the part of the scan
/// that vendor reads. `before` holds what you last trusted for this
/// project. A scan that failed can't be trusted, so it only offers to stay
/// restricted.
pub fn card(
    agent: AgentId,
    vendor: Vendor,
    cwd: &Path,
    scan: &FolderScan,
    before: Option<&[ScannedFile]>,
    now: Timestamp,
) -> Card {
    let id = card_id(agent, &fingerprint(&scan.files));
    let folder = cwd.file_name().map_or_else(
        || cwd.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let choice = |label: &str, description: &str| QuestionOption {
        label: label.to_owned(),
        description: Some(description.to_owned()),
    };
    let mut choices = Vec::new();
    if !scan.failed {
        choices.push(choice(
            TRUST_LABEL,
            "Restarts the agent between turns with the folder's files.",
        ));
    }
    choices.push(choice(
        RESTRICTED_LABEL,
        "This agent carries on without them.",
    ));
    let summary: String = visible(&folder).chars().take(80).collect();
    let event = AttentionEvent {
        id,
        agent,
        kind: AttentionKind::Question,
        summary: format!("Trust this folder to run its own files? {summary}"),
        // No buttons: an inbox shows only the summary, so it sends you to the
        // agent's feed, where the question lists the files.
        options: Vec::new(),
        accepts_text: false,
        created_at: now,
        expires_at: None,
    };
    let item = TimelineItem {
        id: ItemId(format!("maxhq-folder-trust-{id}")),
        agent,
        turn: None,
        parent: None,
        ordinal: 0,
        status: ItemStatus::Waiting,
        started_at: Some(now),
        completed_at: None,
        body: ItemBody::Question {
            attention: id,
            questions: vec![Question {
                id: QUESTION.to_owned(),
                header: Some("Folder trust".to_owned()),
                text: card_text(vendor, cwd, scan, before),
                options: choices,
                multi: false,
                other: false,
                secret: false,
                consent: None,
            }],
            answers: None,
        },
    };
    Card { event, item }
}

/// What a card says. Clients show it as plain text, and all of it goes
/// through [`visible`], since the folder wrote it.
fn card_text(
    vendor: Vendor,
    cwd: &Path,
    scan: &FolderScan,
    before: Option<&[ScannedFile]>,
) -> String {
    let mut text = String::from(match vendor {
        Vendor::Codex => {
            "This folder has files that can run code when an agent starts here. Until you trust \
             it, this agent runs without the folder's .codex settings, MCP servers, hooks and \
             command rules, and without its AGENTS.md. Skills in the folder still load. MaxHQ's \
             own tools still work.\n"
        }
        Vendor::Cursor => {
            "This folder has files that can run code when an agent starts here. Until you trust \
             it, this agent runs with a Cursor data folder of MaxHQ's own, so no MCP server in \
             .cursor/mcp.json starts, even one you approved in Cursor before. Your own MCP \
             servers and hooks, for all your projects, still run, and so do MaxHQ's own tools. \
             Rules and AGENTS.md still load.\n\nCursor \
             has no switch for the folder's .cursor/hooks.json. The Cursor MaxHQ was tested \
             with doesn't run it, trusted or not, but a newer one might.\n"
        }
        _ => {
            "This folder has files that can run code when an agent starts here. Until you trust \
             it, this agent runs without the folder's settings, MCP servers, CLAUDE.md, skills, \
             commands and agents. It also goes without skills MaxHQ delivered here. MCP servers \
             you added yourself, for all your projects or for this one, still start, and so do \
             MaxHQ's own tools.\n"
        }
    });
    text.push_str(&format!(
        "\nFolder: {}\n",
        visible(&cwd.display().to_string())
    ));
    if scan.failed {
        text.push_str(
            "\nSome of these files couldn't be read, so this folder can't be trusted until \
             that's fixed.\n",
        );
    }
    if scan
        .files
        .iter()
        .any(|file| file.name.ends_with(UNFOLLOWED))
    {
        text.push_str(
            "\nSome hooks run commands MaxHQ can't follow to their files. If what those run \
             changes, MaxHQ won't ask again, so read them before you trust this folder.\n",
        );
    }
    if let Some(before) = before {
        text.push_str("\nWhat changed since you last trusted this project:\n");
        for line in changes(&named(before), &named(&scan.files)) {
            text.push_str(&format!("  {}\n", visible(&line)));
        }
    }
    text.push_str("\nFiles:\n");
    for file in scan.files.iter().take(LISTED) {
        text.push_str(&format!("  {}\n", visible(&file.name)));
    }
    if scan.files.len() > LISTED {
        text.push_str(&format!("  and {} more\n", scan.files.len() - LISTED));
    }
    for shown in &scan.shown {
        text.push_str(&format!("\n{}:\n", visible(&shown.name)));
        for line in shown.text.lines() {
            text.push_str(&format!("| {}\n", visible(line)));
        }
        if shown.cut {
            text.push_str(&format!(
                "(cut here: this card shows the first {} KiB)\n",
                SHOWN_BYTES / 1024
            ));
        }
    }
    text
}

/// Each file's name with its digest, as [`changes`] takes them.
fn named(files: &[ScannedFile]) -> Vec<(&str, &str)> {
    files
        .iter()
        .map(|file| (file.name.as_str(), file.digest.as_str()))
        .collect()
}

/// What changed between two lists of names with their digests, one line
/// each.
pub(crate) fn changes(before: &[(&str, &str)], now: &[(&str, &str)]) -> Vec<String> {
    let digests = |pairs: &[(&str, &str)], name: &str| -> Vec<String> {
        pairs
            .iter()
            .filter(|(named, _)| *named == name)
            .map(|(_, digest)| (*digest).to_owned())
            .collect()
    };
    let names: BTreeSet<&str> = before.iter().chain(now).map(|(name, _)| *name).collect();
    names
        .into_iter()
        .filter_map(|name| {
            let (old, new) = (digests(before, name), digests(now, name));
            match (old.is_empty(), new.is_empty()) {
                (false, true) => Some(format!("removed {name}")),
                (true, _) => Some(format!("new {name}")),
                (false, false) if old != new => Some(format!("changed {name}")),
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
