// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Opens Terminal with one line to run, such as a host item's root fix.
//! The line goes into a `.command` file in the app's cache folder, which
//! Terminal runs in a new window and which removes itself as it starts.
//! That needs no permission to control Terminal, and you type any
//! password into Terminal itself, so the app never sees it.
//!
//! On Linux the script opens in the first terminal [`terminal_command`]
//! finds. On Windows the line opens in PowerShell, with no file, as
//! [`powershell`] says.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

/// The script Terminal runs for `line`. Terminal on macOS keeps its window
/// open once the line ends, so you can read what it printed. Terminals
/// elsewhere close theirs, so there the script waits for Enter first.
pub fn command_file(line: &str) -> String {
    let wait = if cfg!(target_os = "macos") {
        ""
    } else {
        WAIT_FOR_ENTER
    };
    format!("#!/bin/sh\nrm -f -- \"$0\"\n{line}\n{wait}")
}

/// The end of the script off macOS, which keeps the window open.
const WAIT_FOR_ENTER: &str = "printf '\\nPress Enter to close this window.'\nread -r _\n";

/// Writes the script for `line` into `dir`, readable and runnable by you
/// alone, under a name nothing there has yet, and gives its path.
pub fn write_command_file(dir: &Path, line: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    let path = dir.join(format!("maxhq-{}-{stamp}.command", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o700);
    }
    let mut file = options.open(&path)?;
    file.write_all(command_file(line).as_bytes())?;
    Ok(path)
}

#[cfg(any(windows, test))]
mod powershell;

/// Opens Terminal on `line`, through a script in `dir` off Windows.
pub async fn open(dir: PathBuf, line: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        let _ = dir;
        powershell::open(&line).await
    }
    #[cfg(not(windows))]
    open_script(dir, line).await
}

/// Opens Terminal, or the terminal found on Linux, on a script for `line`
/// in `dir`.
#[cfg_attr(windows, allow(dead_code))]
async fn open_script(dir: PathBuf, line: String) -> Result<(), String> {
    let path = tokio::task::spawn_blocking(move || write_command_file(&dir, &line))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("Couldn't ready the command for Terminal: {error}"))?;
    if !cfg!(target_os = "macos") {
        return open_found(path);
    }
    let status = tokio::process::Command::new("/usr/bin/open")
        .args(["-a", "Terminal"])
        .arg(&path)
        .status()
        .await
        .map_err(|error| format!("Terminal didn't open: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        let _ = std::fs::remove_file(&path);
        Err(format!("Terminal didn't open: open ended with {status}"))
    }
}

/// Opens the script at `path` in the first terminal found. It doesn't wait,
/// since some terminals run until you close their window, and the runtime
/// reaps the process.
fn open_found(path: PathBuf) -> Result<(), String> {
    let folders: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let terminal = std::env::var_os("TERMINAL");
    let started = match terminal_command(terminal.as_deref(), &folders, &path, is_program) {
        Some(command) => crate::launch::command(&command[0])
            .args(&command[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(drop)
            .map_err(|error| format!("The terminal didn't open: {error}")),
        None => Err("No terminal found. Install one, or set TERMINAL to yours".to_owned()),
    };
    if started.is_err() {
        let _ = std::fs::remove_file(&path);
    }
    started
}

/// The terminals tried after `$TERMINAL`, in order, each with the
/// arguments that come before the program it runs. `x-terminal-emulator`
/// is the one you choose on Debian and Ubuntu.
const TERMINALS: [(&str, &[&str]); 8] = [
    ("x-terminal-emulator", &["-e"]),
    ("gnome-terminal", &["--"]),
    ("konsole", &["-e"]),
    ("xfce4-terminal", &["-x"]),
    ("kitty", &[]),
    ("alacritty", &["-e"]),
    ("wezterm", &["start", "--"]),
    ("xterm", &["-e"]),
];

/// The arguments for a `$TERMINAL` that [`TERMINALS`] doesn't know, which
/// most terminals take as xterm does.
const RUN_ARGS: &[&str] = &["-e"];

/// The command line that opens `script` in a terminal: `terminal`, the
/// `$TERMINAL` setting, if it names a program by its name or path, else
/// the first of [`TERMINALS`] in the `path` folders. A name is looked up
/// in absolute folders only, so a relative entry can't pick a program
/// from wherever the app started. `is_program` says whether a file is one
/// you can run.
fn terminal_command(
    terminal: Option<&OsStr>,
    path: &[PathBuf],
    script: &Path,
    is_program: impl Fn(&Path) -> bool,
) -> Option<Vec<OsString>> {
    let find = |program: &Path| {
        if program
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
        {
            return is_program(program).then(|| program.to_owned());
        }
        path.iter()
            .filter(|folder| folder.is_absolute())
            .map(|folder| folder.join(program))
            .find(|candidate| is_program(candidate))
    };
    let set = terminal
        .filter(|terminal| !terminal.is_empty())
        .and_then(|terminal| {
            let terminal = Path::new(terminal);
            let args = TERMINALS
                .iter()
                .find(|(name, _)| terminal.file_name() == Some(OsStr::new(name)))
                .map_or(RUN_ARGS, |(_, args)| *args);
            Some((find(terminal)?, args))
        });
    let (program, args) = set.or_else(|| {
        TERMINALS
            .iter()
            .find_map(|(name, args)| Some((find(Path::new(name))?, *args)))
    })?;
    let mut command = vec![program.into_os_string()];
    command.extend(args.iter().map(OsString::from));
    command.push(script.into());
    Some(command)
}

/// Whether `path` is a file you can run.
fn is_program(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "ssh -t dev@devbox 'sudo /home/dev/.local/bin/maxd host apply --system'";

    #[test]
    fn the_script_removes_itself_then_runs_the_line_as_given() {
        let runs = format!("#!/bin/sh\nrm -f -- \"$0\"\n{LINE}\n");
        if cfg!(target_os = "macos") {
            assert_eq!(command_file(LINE), runs);
        } else {
            assert_eq!(command_file(LINE), format!("{runs}{WAIT_FOR_ENTER}"));
        }
    }

    const SCRIPT: &str = "/home/<user>/.cache/maxhq/terminal/maxhq-1-2.command";

    /// The command line found with `terminal` set and these `programs` in
    /// the `path` folders.
    fn found(terminal: Option<&str>, path: &[&str], programs: &[&str]) -> Option<Vec<String>> {
        let folders: Vec<PathBuf> = path.iter().map(PathBuf::from).collect();
        let is_program = |candidate: &Path| {
            programs
                .iter()
                .any(|program| candidate == Path::new(program))
        };
        terminal_command(
            terminal.map(OsStr::new),
            &folders,
            Path::new(SCRIPT),
            is_program,
        )
        .map(|command| {
            command
                .into_iter()
                .map(|part| part.into_string().unwrap())
                .collect()
        })
    }

    fn line(parts: &[&str]) -> Option<Vec<String>> {
        Some(parts.iter().map(|part| (*part).to_owned()).collect())
    }

    const PATH: [&str; 2] = ["/usr/local/bin", "/usr/bin"];

    #[test]
    fn your_terminal_setting_comes_first_with_its_own_arguments() {
        assert_eq!(
            found(Some("kitty"), &PATH, &["/usr/bin/kitty", "/usr/bin/xterm"]),
            line(&["/usr/bin/kitty", SCRIPT])
        );
        assert_eq!(
            found(
                Some("wezterm"),
                &PATH,
                &["/usr/local/bin/wezterm", "/usr/bin/wezterm"]
            ),
            line(&["/usr/local/bin/wezterm", "start", "--", SCRIPT])
        );
    }

    #[test]
    fn a_terminal_setting_it_doesnt_know_runs_the_script_as_xterm_would() {
        assert_eq!(
            found(Some("foot"), &PATH, &["/usr/bin/foot", "/usr/bin/xterm"]),
            line(&["/usr/bin/foot", "-e", SCRIPT])
        );
    }

    #[test]
    fn a_terminal_setting_can_be_a_path() {
        assert_eq!(
            found(
                Some("/home/<user>/bin/alacritty"),
                &PATH,
                &["/home/<user>/bin/alacritty", "/usr/bin/xterm"]
            ),
            line(&["/home/<user>/bin/alacritty", "-e", SCRIPT])
        );
    }

    #[test]
    fn without_a_usable_setting_the_list_decides_in_its_order() {
        let installed = [
            "/usr/bin/xterm",
            "/usr/bin/konsole",
            "/usr/bin/gnome-terminal",
        ];
        let expected = line(&["/usr/bin/gnome-terminal", "--", SCRIPT]);
        assert_eq!(found(None, &PATH, &installed), expected);
        assert_eq!(found(Some(""), &PATH, &installed), expected);
        assert_eq!(found(Some("missing"), &PATH, &installed), expected);
        assert_eq!(
            found(
                None,
                &PATH,
                &["/usr/bin/xterm", "/usr/bin/x-terminal-emulator"]
            ),
            line(&["/usr/bin/x-terminal-emulator", "-e", SCRIPT])
        );
        assert_eq!(
            found(None, &PATH, &["/usr/bin/xterm", "/usr/bin/xfce4-terminal"]),
            line(&["/usr/bin/xfce4-terminal", "-x", SCRIPT])
        );
    }

    #[test]
    fn relative_path_folders_are_skipped() {
        assert_eq!(found(None, &["bin", "."], &["bin/xterm", "./xterm"]), None);
        assert_eq!(found(None, &[], &["/usr/bin/xterm"]), None);
    }

    #[test]
    fn each_script_gets_its_own_file_for_you_alone() {
        let dir = std::env::temp_dir().join(format!("maxhq-terminal-{}", std::process::id()));
        let first = write_command_file(&dir, LINE).unwrap();
        let second = write_command_file(&dir, "true").unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read_to_string(&first).unwrap(), command_file(LINE));
        assert_eq!(first.extension().unwrap(), "command");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&first).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
