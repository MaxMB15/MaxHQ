// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Finds an agent's CLI and starts it the way this system runs programs.
//! Every search goes through `os::which`. On Unix that's `PATH`, then the
//! folders installers use, and a file with an execute bit. On Windows it's
//! `PATH` with `PATHEXT` and a `.exe` first, then the folders Windows
//! installers use, and an npm `.cmd` shim looked through to the program it
//! starts.
//!
//! A `.cmd` or `.bat` that can't be looked through runs under `cmd.exe`,
//! which the standard library quotes for, refusing arguments it can't
//! quote. [`batch_refusal`] says why before the holder tries.

use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use maxhq_os::which::{self, Env, Rules};

/// The CLIs with Windows install folders of their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cli {
    /// Claude Code.
    Claude,
    /// Codex.
    Codex,
    /// Cursor's CLI.
    Cursor,
    /// Anything else, such as a local ACP command.
    Other,
}

/// The user's folders the Windows installers use.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowsFolders {
    /// `%USERPROFILE%`.
    pub home: Option<PathBuf>,
    /// `%APPDATA%`, the roaming one.
    pub app_data: Option<PathBuf>,
    /// `%LOCALAPPDATA%`.
    pub local_app_data: Option<PathBuf>,
}

impl WindowsFolders {
    /// This user's.
    pub fn current() -> Self {
        Self {
            home: maxhq_os::home::home_dir(),
            app_data: env::var_os("APPDATA").map(PathBuf::from),
            local_app_data: env::var_os("LOCALAPPDATA").map(PathBuf::from),
        }
    }

    /// Where `cli` may be installed, tried after `PATH`. A service gets a
    /// short `PATH` that misses most of these.
    pub fn of(&self, cli: Cli) -> Vec<PathBuf> {
        let under = |root: &Option<PathBuf>, dir: &str| root.as_deref().map(|root| join(root, dir));
        let own = match cli {
            // The native installer's place comes first, then the old
            // local install's.
            Cli::Claude => vec![
                under(&self.home, r".local\bin"),
                under(&self.home, r".claude\local"),
            ],
            // The installer keeps `cursor-agent.exe` and `agent.exe` in
            // its own folder.
            Cli::Cursor => vec![
                under(&self.local_app_data, "cursor-agent"),
                under(&self.home, r".local\bin"),
            ],
            Cli::Codex | Cli::Other => vec![under(&self.home, r".local\bin")],
        };
        let shared = [
            under(&self.app_data, "npm"),
            under(&self.local_app_data, "pnpm"),
            under(&self.local_app_data, r"Volta\bin"),
            under(&self.home, r".bun\bin"),
            under(&self.home, r"scoop\shims"),
            under(&self.local_app_data, r"Microsoft\WinGet\Links"),
        ];
        own.into_iter().chain(shared).flatten().collect()
    }
}

/// `dir` under `root`, joined with a backslash as Windows joins them.
fn join(root: &Path, dir: &str) -> PathBuf {
    let mut path = root.as_os_str().to_owned();
    if !root.to_string_lossy().ends_with(['\\', '/']) {
        path.push("\\");
    }
    path.push(dir);
    PathBuf::from(path)
}

/// The program `name` from an absolute folder on `path`, then from the
/// install folders: `unix` on Unix, and `cli`'s Windows folders there.
pub fn find(name: &str, path: Option<&OsStr>, unix: &[PathBuf], cli: Cli) -> Option<PathBuf> {
    let extra = match Rules::HOST {
        Rules::Unix => unix.to_vec(),
        Rules::Windows => WindowsFolders::current().of(cli),
    };
    which::find_in(
        Rules::HOST,
        name,
        &extra,
        &Env::with_path(path),
        &which::Host,
    )
}

/// Readies `program` and `args` to start as this system starts them. On
/// Windows an npm shim becomes the program it names, with the script a
/// `node.exe` runs put before the arguments, and a batch file left over
/// is refused, with why, when `cmd.exe` can't take its arguments whole.
/// Unix starts every program as it is.
pub fn ready(program: &mut PathBuf, args: &mut Vec<OsString>) -> Result<(), String> {
    let found = which::launch(program);
    resolve_to(program, args, found);
    match batch_refusal(program, args) {
        Some(reason) => Err(reason),
        None => Ok(()),
    }
}

/// How to start `program`, a path or a bare name, as this system starts
/// it. Unix starts it as it is and searches `PATH` for a bare name itself.
/// Windows finds a bare name through `PATHEXT`, which the standard library
/// doesn't, so `npm` is `npm.cmd`, and then looks through an npm shim.
pub fn launch_of(program: &OsStr) -> which::Launch {
    launch_of_in(
        Rules::HOST,
        program,
        |name| which::find(name, &[]),
        which::launch,
    )
}

/// [`launch_of`] under `rules`, with `find` for a bare name and `launch` to
/// look through what it found.
fn launch_of_in(
    rules: Rules,
    program: &OsStr,
    find: impl FnOnce(&str) -> Option<PathBuf>,
    launch: impl FnOnce(&Path) -> which::Launch,
) -> which::Launch {
    let given = Path::new(program);
    if rules == Rules::Unix {
        return which::Launch::direct(given);
    }
    let text = program.to_string_lossy();
    let bare = !text.contains(['\\', '/', ':']);
    let found = match bare {
        true => find(&text).unwrap_or_else(|| given.to_owned()),
        false => given.to_owned(),
    };
    launch(&found)
}

/// A command that starts `program`, for the caller to add its own
/// arguments to. See [`launch_of`].
pub fn command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    tokio::process::Command::from(std_command(program))
}

/// [`command`] for the standard library's process.
pub fn std_command(program: impl AsRef<OsStr>) -> std::process::Command {
    launch_of(program.as_ref()).command()
}

/// Points `program` at `found`, what it starts, with `found`'s arguments
/// before `args`.
fn resolve_to(program: &mut PathBuf, args: &mut Vec<OsString>, found: which::Launch) {
    if found.program != *program || !found.args.is_empty() {
        *program = found.program;
        args.splice(0..0, found.args);
    }
}

/// `PATH` with `dir` first, then `rest`, joined as this system joins it.
pub fn path_with_first(dir: &Path, rest: Option<&OsStr>) -> OsString {
    path_with_first_in(Rules::HOST, dir, rest)
}

/// [`path_with_first`] under `rules`.
pub fn path_with_first_in(rules: Rules, dir: &Path, rest: Option<&OsStr>) -> OsString {
    let mut first = OsString::from(dir);
    if let Some(rest) = rest.filter(|rest| !rest.is_empty()) {
        first.push(match rules {
            Rules::Unix => ":",
            Rules::Windows => ";",
        });
        first.push(rest);
    }
    first
}

/// The longest command `cmd.exe` reads, in characters.
const CMD_LINE_MAX: usize = 8191;

/// Why `program` can't start with `args`, when it's a batch file that
/// runs under `cmd.exe` and they're arguments `cmd.exe` can't take whole.
/// None for every other program, and on Unix.
pub fn batch_refusal(program: &Path, args: &[OsString]) -> Option<String> {
    batch_refusal_in(Rules::HOST, program, args)
}

/// [`batch_refusal`] under `rules`.
///
/// The standard library quotes a batch file's arguments for `cmd.exe` and
/// refuses a line break, which would end the command early. `cmd.exe`
/// also reads at most [`CMD_LINE_MAX`] characters, and the quoting makes
/// a `%` eight characters long, so a long argument such as JSON or a
/// prompt can be cut. Both are refused here with what to do instead.
pub fn batch_refusal_in(rules: Rules, program: &Path, args: &[OsString]) -> Option<String> {
    if rules != Rules::Windows || !is_batch(program) {
        return None;
    }
    let name = program_name(program);
    let broken = args.iter().any(|arg| {
        arg.as_encoded_bytes()
            .iter()
            .any(|byte| matches!(byte, b'\r' | b'\n' | 0))
    });
    if broken {
        return Some(format!(
            "{name} starts through a batch file, and cmd.exe can't pass it an argument with a \
             line break. Install its .exe build, or the npm package, so MaxHQ can start it \
             directly."
        ));
    }
    if quoted_length(program, args) > CMD_LINE_MAX {
        return Some(format!(
            "{name} starts through a batch file, and its command line is longer than cmd.exe \
             reads. Install its .exe build, or the npm package, so MaxHQ can start it directly."
        ));
    }
    None
}

/// Whether `program` is a batch file, which Windows runs under `cmd.exe`.
fn is_batch(program: &Path) -> bool {
    let text = program.to_string_lossy();
    let name = text.rsplit(['\\', '/']).next().unwrap_or(&text);
    name.rfind('.').is_some_and(|dot| {
        let ext = &name[dot..];
        ext.eq_ignore_ascii_case(".cmd") || ext.eq_ignore_ascii_case(".bat")
    })
}

/// The file name of `program`, for a reason.
fn program_name(program: &Path) -> String {
    let text = program.to_string_lossy();
    text.rsplit(['\\', '/']).next().unwrap_or(&text).to_owned()
}

/// About how long the command `cmd.exe` reads for `program` and `args` is,
/// quoted as the standard library quotes them: the program and each
/// argument in quotes, each `"` doubled, and each `%` written as
/// `%%cd:~,%` so `cmd.exe` doesn't expand it.
fn quoted_length(program: &Path, args: &[OsString]) -> usize {
    let quoted = |text: &str| -> usize {
        let extra: usize = text
            .chars()
            .map(|c| match c {
                '"' => 1,
                '%' => 7,
                _ => 0,
            })
            .sum();
        text.encode_utf16().count() + extra + 2
    };
    let script = quoted(&program.to_string_lossy());
    let args: usize = args
        .iter()
        .map(|arg| 1 + quoted(&arg.to_string_lossy()))
        .sum();
    // The outer pair of quotes around the whole command.
    script + args + 2
}

#[cfg(test)]
mod tests;
