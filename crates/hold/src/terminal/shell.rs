// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Shell integration: startup files that make zsh, bash, fish and, on
//! Windows, PowerShell in the drawer mark their prompts and commands, as
//! `marks.rs` reads them, and say their folder at each prompt. The user's
//! own startup files run as they would:
//!
//! - zsh starts with ZDOTDIR at a folder of MaxHQ's, whose `.zshenv`,
//!   `.zprofile`, `.zshrc` and `.zlogin` each run the user's with ZDOTDIR
//!   as the user had it. The hooks go in after the user's `.zshrc`.
//! - bash starts as `bash --init-file <file>`. The file does what a login
//!   shell would, `/etc/profile` and then the first of `~/.bash_profile`,
//!   `~/.bash_login` and `~/.profile`, then puts the hooks in
//!   `PROMPT_COMMAND` and the DEBUG trap around the user's. Git Bash on
//!   Windows reads the same file, by a path with forward slashes, and is
//!   told it's interactive with `-i`, as a console may not look like a
//!   terminal to it.
//! - fish starts as `fish -l --init-command 'source <file>'`, which runs
//!   after the user's config. A snippet in the folder's
//!   `fish/vendor_conf.d`, which fish finds through XDG_DATA_DIRS, runs
//!   before the user's `config.fish` and puts XDG_DATA_DIRS back.
//! - PowerShell, 7 or Windows PowerShell 5.1, starts as
//!   `-NoLogo -NoExit -Command <script>`, which runs after the user's
//!   profile. The script is read from the file and run as text, not
//!   dot-sourced, so an execution policy that blocks script files doesn't
//!   block it. It wraps the `prompt` function and `PSConsoleHostReadLine`.
//!   The user's profile runs with the nonce in its environment, as it runs
//!   first.
//!
//! A command `maxd` sends for the chat runs with PAGER and GIT_PAGER set to
//! `cat` while a file in the terminal's folder says so, as
//! [`super::Terminals::no_pager`] writes it. The shell gets the file's path
//! as it gets the nonce, and gives the values back at its next prompt. What
//! you type in the drawer keeps your pager.
//!
//! The marks carry a nonce for the terminal, which the shell gets in its
//! environment and keeps in a variable, out of what it starts. The user's
//! startup files run without it but for `/etc/zshenv` and fish's `conf.d`
//! snippets. It stays where the shell's first environment shows, as
//! `/proc/<pid>/environ` and `ps eww` do, and bash's PS1 holds it. The
//! files go in a folder named by what they hold, so a new `maxd` never
//! changes the files a running shell reads, and one that finds a file in it
//! changed puts it back.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{marker, TermLaunch};
use crate::{files, secret};

/// The variable that gives the shell its nonce.
const NONCE: &str = "MAXHQ_SHELL_NONCE";

/// The variable that gives the shell the path of the file that turns the
/// pager off.
const PAGER_FLAG: &str = "MAXHQ_SHELL_PAGER";

/// That file's name in the terminal's folder.
pub(super) const NO_PAGER: &str = "no-pager";

/// The variable that gives zsh the user's own ZDOTDIR.
const USER_ZDOTDIR: &str = "MAXHQ_USER_ZDOTDIR";

/// Where fish looks for vendor snippets, among other folders.
const XDG_DATA_DIRS: &str = "XDG_DATA_DIRS";

/// The variable that gives fish the user's own XDG_DATA_DIRS, empty when
/// it had none.
const USER_XDG_DATA_DIRS: &str = "MAXHQ_USER_XDG_DATA_DIRS";

/// What XDG_DATA_DIRS means when it isn't set.
const XDG_DEFAULT: &str = "/usr/local/share:/usr/share";

/// The files, by path in the folder.
const FILES: &[(&str, &str)] = &[
    (".zshenv", include_str!("shell/zshenv.zsh")),
    (".zprofile", include_str!("shell/zprofile.zsh")),
    (".zshrc", include_str!("shell/zshrc.zsh")),
    (".zlogin", include_str!("shell/zlogin.zsh")),
    ("bash.sh", include_str!("shell/bash.sh")),
    ("fish.fish", include_str!("shell/fish.fish")),
    (
        "fish/vendor_conf.d/maxhq-shell-integration.fish",
        include_str!("shell/fish-vendor.fish"),
    ),
];

/// The files only Windows has a shell for.
const WINDOWS_FILES: &[(&str, &str)] = &[("powershell.ps1", include_str!("shell/powershell.ps1"))];

/// The files this OS's shells read. A Unix folder holds no PowerShell
/// script, so its name and contents stay as they were before Windows.
fn os_files() -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    let windows: &[(&str, &str)] = if cfg!(windows) { WINDOWS_FILES } else { &[] };
    FILES.iter().chain(windows)
}

/// How a folder or file being written is named until it's done. One a
/// `maxd` that stopped part way left goes in [`sweep`].
const TEMP: &str = ".shell-";

/// How old a leftover is before [`sweep`] takes it, so it never takes one
/// a write is still at.
const LEFTOVER_AGE: Duration = Duration::from_secs(60);

// PowerShell is the shell's own name, which ends as this one does.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shell {
    Zsh,
    Bash,
    /// bash from Git for Windows.
    GitBash,
    Fish,
    /// PowerShell 7 or Windows PowerShell, on Windows only.
    PowerShell,
}

impl Shell {
    fn of(program: &Path) -> Option<Self> {
        Self::named(program.file_name()?.to_str()?, cfg!(windows))
    }

    /// The shell a program's file name names. Windows reads names without
    /// regard to case, with `.exe` or without.
    fn named(name: &str, windows: bool) -> Option<Self> {
        if !windows {
            return match name {
                "zsh" => Some(Self::Zsh),
                "bash" => Some(Self::Bash),
                "fish" => Some(Self::Fish),
                _ => None,
            };
        }
        let name = name.to_ascii_lowercase();
        match name.strip_suffix(".exe").unwrap_or(&name) {
            "bash" => Some(Self::GitBash),
            "pwsh" | "powershell" => Some(Self::PowerShell),
            _ => None,
        }
    }
}

/// Whether `program` is PowerShell.
pub(super) fn is_powershell(program: &Path) -> bool {
    Shell::of(program) == Some(Shell::PowerShell)
}

/// How a drawer shell running `program` starts before [`integrate`].
pub(super) fn start_args(program: &Path) -> Vec<OsString> {
    start_args_for(Shell::of(program))
}

/// A login shell, as a terminal app starts one. Git Bash is told it's
/// interactive too, as editors start it, and PowerShell has no login
/// shell, only its banner to leave out.
fn start_args_for(shell: Option<Shell>) -> Vec<OsString> {
    let args: &[&str] = match shell {
        Some(Shell::PowerShell) => &["-NoLogo"],
        Some(Shell::GitBash) => &["-l", "-i"],
        _ => &["-l"],
    };
    args.iter().map(OsString::from).collect()
}

/// How a `!` run of `command` in `program` starts.
pub(super) fn run_args(program: &Path, command: &str) -> Vec<OsString> {
    run_args_for(Shell::of(program), command)
}

fn run_args_for(shell: Option<Shell>, command: &str) -> Vec<OsString> {
    if shell == Some(Shell::PowerShell) {
        return vec![
            "-NoLogo".into(),
            "-Command".into(),
            powershell_run(command).into(),
        ];
    }
    vec!["-ilc".into(), marker::after_marker(command).into()]
}

/// The script for a PowerShell run: the marker, no pager and `command`,
/// as [`marker::after_marker`] gives a POSIX shell, then an exit with the
/// code of the program that failed, or 1 for a failure of PowerShell's
/// own. PowerShell would exit with 1 for either. The code starts at 0, so
/// one a profile left doesn't count. The exit goes on a line of its own,
/// so a comment at the end of `command` can't take it in.
fn powershell_run(command: &str) -> String {
    let body = String::from_utf8_lossy(&marker::MARKER[2..marker::MARKER.len() - 1]);
    format!(
        "[Console]::Write([char]27 + ']{body}' + [char]7); $global:LASTEXITCODE = 0; \
         $env:PAGER = 'cat'; $env:GIT_PAGER = 'cat'; {command}\n\
         if (-not $?) {{ if ($LASTEXITCODE) {{ exit $LASTEXITCODE }} else {{ exit 1 }} }}"
    )
}

/// `text` in single quotes, as PowerShell reads them. PowerShell takes the
/// curly quotes for one too, so each of them is doubled as well.
pub(super) fn powershell_quoted(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('\'');
    for c in text.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
            quoted.push(c);
        }
        quoted.push(c);
    }
    quoted.push('\'');
    quoted
}

/// Makes `launch`, a drawer shell, mark its prompts and commands when it's
/// zsh, bash, fish or, on Windows, PowerShell, with the files in a folder
/// under `dir`. Returns whether it will.
pub(super) fn integrate(launch: &mut TermLaunch, dir: &Path) -> io::Result<bool> {
    let Some(shell) = Shell::of(&launch.program).filter(|_| !launch.run) else {
        return Ok(false);
    };
    integrate_as(launch, dir, shell)
}

fn integrate_as(launch: &mut TermLaunch, dir: &Path, shell: Shell) -> io::Result<bool> {
    let dir = std::path::absolute(dir)?;
    let folder = write_files(&dir)?;
    match shell {
        Shell::Zsh => {
            let user = env_value(launch, "ZDOTDIR").filter(|dir| !dir.is_empty());
            if let Some(user) = user {
                launch.env.push((USER_ZDOTDIR.into(), user));
            }
            launch.env.push(("ZDOTDIR".into(), folder.into()));
        }
        Shell::Bash => {
            launch.args = vec!["--init-file".into(), folder.join("bash.sh").into()];
        }
        Shell::GitBash => {
            let Some(file) = folder
                .join("bash.sh")
                .to_str()
                .map(|f| f.replace('\\', "/"))
            else {
                return Ok(false);
            };
            launch.args = vec!["--init-file".into(), file.into(), "-i".into()];
        }
        Shell::Fish => {
            let Some(file) = folder.join("fish.fish").to_str().map(fish_quoted) else {
                return Ok(false);
            };
            // A folder with a colon can't go in a list of them, so then the
            // nonce waits for the init command.
            if !folder.as_os_str().to_string_lossy().contains(':') {
                let user = env_value(launch, XDG_DATA_DIRS).filter(|dirs| !dirs.is_empty());
                let mut dirs = folder.clone().into_os_string();
                dirs.push(":");
                dirs.push(user.as_deref().unwrap_or(OsStr::new(XDG_DEFAULT)));
                launch
                    .env
                    .push((USER_XDG_DATA_DIRS.into(), user.unwrap_or_default()));
                launch.env.push((XDG_DATA_DIRS.into(), dirs));
            }
            launch.args = vec![
                "-l".into(),
                "--init-command".into(),
                format!("source {file}").into(),
            ];
        }
        Shell::PowerShell => {
            let Some(file) = folder
                .join("powershell.ps1")
                .to_str()
                .map(powershell_quoted)
            else {
                return Ok(false);
            };
            // Read as text, a script block runs whatever the execution
            // policy says of script files.
            let script = format!(". ([scriptblock]::Create([IO.File]::ReadAllText({file})))");
            launch.args = vec![
                "-NoLogo".into(),
                "-NoExit".into(),
                "-Command".into(),
                script.into(),
            ];
        }
    }
    let nonce = secret::new_nonce()?;
    launch.env.push((NONCE.into(), nonce.clone().into()));
    launch.nonce = Some(nonce);
    let flag = dir.join(launch.terminal.to_string()).join(NO_PAGER);
    launch.env.push((PAGER_FLAG.into(), flag.into()));
    Ok(true)
}

/// What `key` is for the program: as `launch` sets it, or as `maxd` has it.
fn env_value(launch: &TermLaunch, key: &str) -> Option<OsString> {
    (launch.env.iter().rev())
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
        .or_else(|| std::env::var_os(key))
}

/// `text` in single quotes, as fish reads them.
fn fish_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\\', r"\\").replace('\'', r"\'"))
}

/// The folder for the files under `dir`, named by them.
fn folder_in(dir: &Path) -> PathBuf {
    // FNV-1a over the names and contents.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (name, text) in os_files() {
        for byte in [name.as_bytes(), &[0], text.as_bytes(), &[0]].concat() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    dir.join(format!("shell-{hash:016x}"))
}

/// The folder with the files under `dir`: written whole when it isn't
/// there yet, and with any file in it that isn't as written put back.
fn write_files(dir: &Path) -> io::Result<PathBuf> {
    let folder = folder_in(dir);
    match fs::symlink_metadata(&folder) {
        Ok(meta) if meta.is_dir() => {
            mend(dir, &folder)?;
            return Ok(folder);
        }
        // Nothing of MaxHQ's but the folder goes there.
        Ok(_) => fs::remove_file(&folder)?,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    files::private_dir_all(dir)?;
    let temp = temp_in(dir)?;
    files::private_dir(&temp)?;
    let written = os_files()
        .try_for_each(|(name, text)| write_file(&temp.join(name), text))
        .and_then(|()| fs::rename(&temp, &folder));
    match written {
        Ok(()) => Ok(folder),
        Err(err) => {
            let _ = fs::remove_dir_all(&temp);
            // Another shell starting at the same time put it there first.
            if folder.is_dir() {
                mend(dir, &folder)?;
                Ok(folder)
            } else {
                Err(err)
            }
        }
    }
}

/// A new name under `dir` for something being written.
fn temp_in(dir: &Path) -> io::Result<PathBuf> {
    Ok(dir.join(format!("{TEMP}{}", secret::new_nonce()?)))
}

/// Writes `text` to `path`, making the folders it goes in.
fn write_file(path: &Path, text: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        files::private_dir_all(parent)?;
    }
    fs::write(path, text)
}

/// Puts back each file in `folder` that isn't as written, as one cut short
/// or changed: written whole under `dir` first, then renamed over it, so a
/// shell starting meanwhile reads the old file or the new one.
fn mend(dir: &Path, folder: &Path) -> io::Result<()> {
    for (name, text) in os_files() {
        let path = folder.join(name);
        if fs::read(&path).is_ok_and(|bytes| bytes == text.as_bytes()) {
            continue;
        }
        let temp = temp_in(dir)?;
        let put = write_file(&temp, text).and_then(|()| {
            if let Some(parent) = path.parent() {
                files::private_dir_all(parent)?;
            }
            fs::rename(&temp, &path)
        });
        if put.is_err() {
            let _ = fs::remove_file(&temp);
        }
        put?;
    }
    Ok(())
}

/// Deletes what a write under `dir` left when its `maxd` stopped part way,
/// once it's a minute old. The folders of earlier files stay, as a shell
/// that started with them may read them yet.
pub(super) fn sweep(dir: &Path) -> io::Result<()> {
    sweep_older(dir, LEFTOVER_AGE)
}

fn sweep_older(dir: &Path, age: Duration) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().starts_with(TEMP) {
            continue;
        }
        // Of the entry itself, not what a link points at.
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let old = (meta.modified().ok())
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|elapsed| elapsed >= age);
        if old {
            let path = entry.path();
            let _ = if meta.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::ids::TerminalId;

    use super::*;

    fn launch(program: &str) -> TermLaunch {
        let mut launch = TermLaunch::shell(TerminalId::generate(), "/tmp".into(), 80, 24);
        launch.program = program.into();
        launch
    }

    fn scratch(name: &str) -> PathBuf {
        let id = TerminalId::generate().0.simple().to_string();
        std::env::temp_dir().join(format!("hold-{name}-{}", &id[20..]))
    }

    #[cfg(unix)]
    fn env<'a>(launch: &'a TermLaunch, key: &str) -> Option<&'a OsString> {
        (launch.env.iter())
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    // zsh and fish are Unix's, and Windows takes a bash as Git Bash.
    #[cfg(unix)]
    #[test]
    fn each_shell_starts_with_its_files_and_a_nonce() {
        let dir = scratch("shell");
        let zsh = {
            let mut launch = launch("/bin/zsh");
            assert!(integrate(&mut launch, &dir).unwrap());
            launch
        };
        assert_eq!(zsh.args, [OsString::from("-l")]);
        let folder = PathBuf::from(env(&zsh, "ZDOTDIR").unwrap());
        for (name, text) in os_files() {
            assert_eq!(fs::read_to_string(folder.join(name)).unwrap(), *text);
        }
        let nonce = zsh.nonce.clone().unwrap();
        assert_eq!(nonce.len(), 32);
        assert_eq!(env(&zsh, NONCE), Some(&OsString::from(&nonce)));
        let flag = std::path::absolute(&dir)
            .unwrap()
            .join(zsh.terminal.to_string())
            .join(NO_PAGER);
        assert_eq!(env(&zsh, PAGER_FLAG), Some(&flag.into()));

        let mut bash = launch("/usr/local/bin/bash");
        assert!(integrate(&mut bash, &dir).unwrap());
        assert_eq!(
            bash.args,
            [OsString::from("--init-file"), folder.join("bash.sh").into()]
        );
        assert_ne!(bash.nonce, zsh.nonce);

        let mut fish = launch("fish");
        fish.env.push((XDG_DATA_DIRS.into(), "/opt/share".into()));
        assert!(integrate(&mut fish, &dir).unwrap());
        let source = format!("source '{}'", folder.join("fish.fish").display());
        assert_eq!(
            fish.args,
            [OsString::from("-l"), "--init-command".into(), source.into()]
        );
        // fish finds the snippet first, which puts back the user's folders.
        let dirs = format!("{}:/opt/share", folder.display());
        assert_eq!(env_value(&fish, XDG_DATA_DIRS), Some(dirs.into()));
        assert_eq!(env(&fish, USER_XDG_DATA_DIRS), Some(&"/opt/share".into()));
        let mut bare = launch("/usr/bin/fish");
        bare.env.push((XDG_DATA_DIRS.into(), "".into()));
        assert!(integrate(&mut bare, &dir).unwrap());
        let dirs = format!("{}:{XDG_DEFAULT}", folder.display());
        assert_eq!(env_value(&bare, XDG_DATA_DIRS), Some(dirs.into()));
        assert_eq!(env(&bare, USER_XDG_DATA_DIRS), Some(&"".into()));
        // One folder, written once.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_isnt_as_written_is_put_back() {
        let dir = scratch("mend");
        let folder = write_files(&dir).unwrap();
        let rc = folder.join(".zshrc");
        let snippet = folder.join(FILES[6].0);
        fs::write(&rc, "cut sho").unwrap();
        fs::remove_file(&snippet).unwrap();
        fs::remove_dir(snippet.parent().unwrap()).unwrap();
        assert_eq!(write_files(&dir).unwrap(), folder);
        for (name, text) in os_files() {
            assert_eq!(fs::read_to_string(folder.join(name)).unwrap(), *text);
        }
        // Nothing it wrote along the way stays.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_sweep_takes_only_old_leftovers() {
        let dir = scratch("sweep");
        let folder = write_files(&dir).unwrap();
        let earlier = dir.join("shell-0123456789abcdef");
        files::private_dir(&earlier).unwrap();
        let half = dir.join(format!("{TEMP}half"));
        files::private_dir(&half).unwrap();
        fs::write(half.join(".zshrc"), "cut sho").unwrap();
        fs::write(dir.join(format!("{TEMP}file")), "cut sho").unwrap();
        let names = || {
            let mut names: Vec<_> = (fs::read_dir(&dir).unwrap())
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            names
        };
        let mut kept = vec![
            "shell-0123456789abcdef".to_owned(),
            folder.file_name().unwrap().to_str().unwrap().to_owned(),
        ];
        kept.sort();
        // A fresh one may be a write still going on.
        sweep(&dir).unwrap();
        assert_eq!(names().len(), 4);
        sweep_older(&dir, Duration::ZERO).unwrap();
        assert_eq!(names(), kept);
        sweep(&scratch("nothing")).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn other_programs_and_runs_start_as_they_were() {
        let dir = scratch("noshell");
        for mut launch in [
            launch("/bin/sh"),
            launch("/usr/bin/nu"),
            launch("/bin/zsh5"),
            TermLaunch::run(TerminalId::generate(), "/tmp".into(), "ls", 80, 24),
        ] {
            let was = launch.clone();
            assert!(!integrate(&mut launch, &dir).unwrap());
            assert_eq!(launch, was);
        }
        assert!(!dir.exists());
    }

    #[test]
    fn fish_reads_quotes_back_as_they_were() {
        assert_eq!(fish_quoted(r"/a b/it's\x"), r"'/a b/it\'s\\x'");
    }
    #[test]
    fn windows_names_git_bash_and_powershell_whatever_the_case() {
        for (name, shell) in [
            ("bash.exe", Shell::GitBash),
            ("BASH.EXE", Shell::GitBash),
            ("pwsh.exe", Shell::PowerShell),
            ("pwsh", Shell::PowerShell),
            ("PowerShell.exe", Shell::PowerShell),
        ] {
            assert_eq!(Shell::named(name, true), Some(shell), "{name}");
        }
        assert_eq!(Shell::named("cmd.exe", true), None);
        assert_eq!(Shell::named("wsl.exe", true), None);
        // Elsewhere, PowerShell and a name with `.exe` aren't integrated.
        assert_eq!(Shell::named("bash", false), Some(Shell::Bash));
        assert_eq!(Shell::named("pwsh", false), None);
        assert_eq!(Shell::named("bash.exe", false), None);
    }

    #[test]
    fn git_bash_and_powershell_start_and_run_their_own_way() {
        let args = |args: &[&str]| args.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(start_args_for(None), args(&["-l"]));
        assert_eq!(start_args_for(Some(Shell::Bash)), args(&["-l"]));
        assert_eq!(start_args_for(Some(Shell::GitBash)), args(&["-l", "-i"]));
        assert_eq!(start_args_for(Some(Shell::PowerShell)), args(&["-NoLogo"]));
        let posix = marker::after_marker("ls");
        assert_eq!(
            run_args_for(Some(Shell::GitBash), "ls"),
            args(&["-ilc", &posix])
        );
        let run = powershell_run("git log # all of it");
        assert_eq!(
            run_args_for(Some(Shell::PowerShell), "git log # all of it"),
            args(&["-NoLogo", "-Command", &run])
        );
        let (first, exit) = run.split_once('\n').unwrap();
        assert_eq!(
            first,
            "[Console]::Write([char]27 + ']7346;maxhq-run' + [char]7); \
             $global:LASTEXITCODE = 0; $env:PAGER = 'cat'; $env:GIT_PAGER = 'cat'; \
             git log # all of it"
        );
        assert!(exit.starts_with("if (-not $?)"), "{exit}");

        let dir = scratch("windows");
        let folder = write_files(&dir).unwrap();
        let mut bash = launch("bash.exe");
        assert!(integrate_as(&mut bash, &dir, Shell::GitBash).unwrap());
        let file = folder.join("bash.sh").to_str().unwrap().replace('\\', "/");
        assert_eq!(bash.args, args(&["--init-file", &file, "-i"]));
        let mut pwsh = launch("pwsh.exe");
        assert!(integrate_as(&mut pwsh, &dir, Shell::PowerShell).unwrap());
        let file = folder.join("powershell.ps1");
        let script = format!(
            ". ([scriptblock]::Create([IO.File]::ReadAllText('{}')))",
            file.display()
        );
        assert_eq!(
            pwsh.args,
            args(&["-NoLogo", "-NoExit", "-Command", &script])
        );
        assert_eq!(pwsh.nonce.as_ref().map(String::len), Some(32));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn powershell_reads_its_quotes_back_as_they_were() {
        assert_eq!(powershell_quoted(r"C:\it's"), r"'C:\it''s'");
        assert_eq!(
            powershell_quoted("a\u{2019}b\u{2018}"),
            "'a\u{2019}\u{2019}b\u{2018}\u{2018}'"
        );
        assert_eq!(powershell_quoted(""), "''");
    }

    #[test]
    fn powershells_marks_pass_the_holders_filter() {
        use super::super::marks::{Filter, Kind};

        let nonce = "0123abcd";
        let mark = |body: &str| format!("\x1b]133;{body};maxhq={nonce}\x07");
        let cwd = "\x1b]7;file://<pc>/C:/Users/%3Cuser%3E/my%20app\x07";
        // As the script prints them: the folder and A before the prompt,
        // B after it, C with the line once it's read, D at the next prompt.
        let output = [
            format!("{cwd}{}PS C:\\> {}", mark("A"), mark("B")),
            format!("echo a;b\r\n{}a\r\nb\r\n", mark("C;echo a%3Bb")),
            format!("{}{cwd}{}PS C:\\> {}", mark("D;0"), mark("A"), mark("B")),
            format!(" secret\r\n{}", mark("C;")),
            format!("{}{cwd}{}", mark("D;1"), mark("A")),
        ]
        .concat();
        let mut filter = Filter::new(Some(nonce));
        let (mut out, mut marks) = (Vec::new(), Vec::new());
        for chunk in output.as_bytes().chunks(7) {
            filter.feed(chunk, &mut out, &mut marks);
        }
        let kinds: Vec<_> = marks.into_iter().map(|(_, kind)| kind).collect();
        assert_eq!(
            kinds,
            [
                Kind::Prompt,
                Kind::Input,
                Kind::Command(Some("echo a;b".into())),
                Kind::Done(Some(0)),
                Kind::Prompt,
                Kind::Input,
                Kind::Command(None),
                Kind::Done(Some(1)),
                Kind::Prompt,
            ]
        );
        let out = String::from_utf8(out).unwrap();
        assert!(!out.contains(nonce), "{out}");
        assert!(out.contains(cwd), "the folder passes on");
    }
}
