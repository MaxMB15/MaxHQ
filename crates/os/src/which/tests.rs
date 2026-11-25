// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::collections::HashMap;

use super::*;

/// Files by path, with their mode and text. Windows paths compare without
/// case and with either slash, as NTFS does.
#[derive(Default)]
struct Files {
    files: HashMap<String, (u32, String)>,
    windows: bool,
}

impl Files {
    fn unix() -> Self {
        Self::default()
    }

    fn windows() -> Self {
        Self {
            windows: true,
            ..Self::default()
        }
    }

    fn key(&self, path: &Path) -> String {
        let path = path.to_string_lossy();
        if self.windows {
            path.to_lowercase().replace('/', "\\")
        } else {
            // A Windows host joins with `\`, even under Unix rules.
            path.replace('\\', "/")
        }
    }

    fn with(mut self, path: &str, mode: u32, text: &str) -> Self {
        let key = self.key(Path::new(path));
        self.files.insert(key, (mode, text.to_owned()));
        self
    }

    fn program(self, path: &str) -> Self {
        self.with(path, 0o755, "")
    }
}

impl Fs for Files {
    fn file_mode(&self, path: &Path) -> Option<u32> {
        self.files.get(&self.key(path)).map(|(mode, _)| *mode)
    }

    fn read_text(&self, path: &Path) -> Option<String> {
        self.files
            .get(&self.key(path))
            .map(|(_, text)| text.clone())
    }
}

fn env(path: &str) -> Env {
    Env {
        path: Some(path.into()),
        pathext: None,
    }
}

fn windows(name: &str, path: &str, files: &Files) -> Option<String> {
    find_in(Rules::Windows, name, &[], &env(path), files)
        .map(|found| found.to_string_lossy().into_owned())
}

// Unix.

#[test]
fn unix_searches_path_then_the_extra_folders() {
    let files = Files::unix()
        .program("/usr/bin/git")
        .program("/opt/tools/claude");
    let extra = [PathBuf::from("/opt/tools")];
    let env = env("/usr/local/bin:/usr/bin");
    assert_eq!(
        find_in(Rules::Unix, "git", &extra, &env, &files),
        Some("/usr/bin/git".into())
    );
    assert_eq!(
        find_in(Rules::Unix, "claude", &extra, &env, &files),
        Some("/opt/tools/claude".into())
    );
    assert_eq!(find_in(Rules::Unix, "codex", &extra, &env, &files), None);
}

#[test]
fn unix_needs_an_execute_bit_and_an_absolute_folder() {
    let files = Files::unix()
        .with("/usr/bin/notes", 0o644, "")
        .program("bin/tool");
    let env = env("bin:/usr/bin");
    assert_eq!(find_in(Rules::Unix, "notes", &[], &env, &files), None);
    assert_eq!(find_in(Rules::Unix, "tool", &[], &env, &files), None);
}

#[test]
fn unix_starts_what_it_found_as_it_is() {
    let files = Files::unix().program("/usr/bin/codex");
    let launch = launch_in(Rules::Unix, Path::new("/usr/bin/codex"), &env(""), &files);
    assert_eq!(launch, Launch::direct("/usr/bin/codex"));
}

// Windows: finding.

#[test]
fn windows_splits_path_at_semicolons_outside_quotes() {
    let dirs = windows::split_paths(OsStr::new(r#"C:\a;;"C:\semi;colon";C:\b"#));
    assert_eq!(
        dirs,
        [
            PathBuf::from(r"C:\a"),
            PathBuf::from(r"C:\semi;colon"),
            PathBuf::from(r"C:\b"),
        ]
    );
}

#[test]
fn windows_knows_an_absolute_folder() {
    for dir in [
        r"C:\tools",
        "c:/tools",
        r"\\server\share\bin",
        "//server/share",
    ] {
        assert!(windows::is_absolute(Path::new(dir)), "{dir}");
    }
    for dir in [r"tools", r"\tools", "C:tools", "", "."] {
        assert!(!windows::is_absolute(Path::new(dir)), "{dir}");
    }
}

#[test]
fn windows_prefers_an_exe_in_the_same_folder() {
    let files = Files::windows()
        .program(r"C:\npm\codex.cmd")
        .program(r"C:\npm\codex.exe");
    assert_eq!(
        windows("codex", r"C:\npm", &files).as_deref(),
        Some(r"C:\npm\codex.exe")
    );
}

#[test]
fn windows_keeps_the_path_order_between_folders() {
    let files = Files::windows()
        .program(r"C:\npm\codex.cmd")
        .program(r"C:\later\codex.exe");
    assert_eq!(
        windows("codex", r"C:\npm;C:\later", &files).as_deref(),
        Some(r"C:\npm\codex.cmd")
    );
}

#[test]
fn windows_runs_only_what_pathext_names() {
    // npm writes a shell script and a PowerShell script beside the shim.
    let files = Files::windows()
        .program(r"C:\npm\codex")
        .program(r"C:\npm\codex.ps1")
        .program(r"D:\tools\codex.cmd");
    assert_eq!(
        windows("codex", r"C:\npm;D:\tools", &files).as_deref(),
        Some(r"D:\tools\codex.cmd")
    );
    let only_exe = Env {
        path: Some(r"D:\tools".into()),
        pathext: Some(".EXE".into()),
    };
    assert_eq!(
        find_in(Rules::Windows, "codex", &[], &only_exe, &files),
        None
    );
}

#[test]
fn windows_takes_a_name_with_its_extension_as_it_is() {
    let files = Files::windows()
        .program(r"C:\node\node.exe")
        .program(r"C:\node\node.exe.cmd");
    assert_eq!(
        windows("node.exe", r"C:\node", &files).as_deref(),
        Some(r"C:\node\node.exe")
    );
}

#[test]
fn windows_compares_extensions_without_case() {
    let files = Files::windows().program(r"C:\Tools\GIT.EXE");
    assert!(windows("git", r"C:\Tools", &files).is_some());
    assert!(windows::runs(Path::new(r"C:\Tools\GIT.EXE"), None));
    assert!(windows::runs(
        Path::new(r"C:\Tools\run.Cmd"),
        Some(".exe;.CMD".as_ref())
    ));
    assert!(!windows::runs(Path::new(r"C:\Tools\notes.txt"), None));
    assert!(!windows::runs(Path::new(r"C:\Tools.d\git"), None));
}

#[test]
fn windows_skips_relative_folders_and_searches_the_extra_ones() {
    let files = Files::windows()
        .program(r"bin\claude.exe")
        .program(r"C:\Users\me\.local\bin\claude.exe");
    let extra = [PathBuf::from(r"C:\Users\me\.local\bin")];
    assert_eq!(
        find_in(Rules::Windows, "claude", &extra, &env("bin"), &files),
        Some(PathBuf::from(r"C:\Users\me\.local\bin\claude.exe"))
    );
}

// Windows: npm shims.

/// What npm 7 and later write for a package whose `bin` is a script.
const NPM_SHIM: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n\
:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  \
SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  \
SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || \
title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@openai\\codex\\bin\\codex.js\" %*\r\n";

/// What npm 6 wrote.
const OLD_NPM_SHIM: &str = "@IF EXIST \"%~dp0\\node.exe\" (\r\n  \
\"%~dp0\\node.exe\"  \"%~dp0\\node_modules\\@anthropic-ai\\claude-code\\cli.js\" %*\r\n\
) ELSE (\r\n  @SETLOCAL\r\n  @SET PATHEXT=%PATHEXT:;.JS;=;%\r\n  \
node  \"%~dp0\\node_modules\\@anthropic-ai\\claude-code\\cli.js\" %*\r\n)\r\n";

/// What npm writes for a package whose `bin` is a native program.
const EXE_SHIM: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n\
:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\"%dp0%\\node_modules\\tool\\bin\\tool.exe\"   %*\r\n";

/// What pnpm writes, with the package a folder up.
const PNPM_SHIM: &str = "@SETLOCAL\r\n@IF EXIST \"%~dp0\\node.exe\" (\r\n  \
\"%~dp0\\node.exe\"  \"%~dp0\\..\\pkg\\bin\\cli.js\" %*\r\n) ELSE (\r\n  \
@SET PATHEXT=%PATHEXT:;.JS;=;%\r\n  node  \"%~dp0\\..\\pkg\\bin\\cli.js\" %*\r\n)\r\n";

fn shim_launch(shim: &str, files: &Files, path: &str) -> Launch {
    launch_in(Rules::Windows, Path::new(shim), &env(path), files)
}

#[test]
fn an_npm_shim_runs_its_script_through_node_on_path() {
    let files = Files::windows()
        .with(r"C:\npm\codex.cmd", 0o600, NPM_SHIM)
        .program(r"C:\npm\node_modules\@openai\codex\bin\codex.js")
        .program(r"C:\Program Files\nodejs\node.exe");
    assert_eq!(
        shim_launch(r"C:\npm\codex.cmd", &files, r"C:\Program Files\nodejs"),
        Launch {
            program: r"C:\Program Files\nodejs\node.exe".into(),
            args: vec![r"C:\npm\node_modules\@openai\codex\bin\codex.js".into()],
        }
    );
}

#[test]
fn an_npm_shim_prefers_the_node_beside_it() {
    let files = Files::windows()
        .with(r"C:\npm\claude.cmd", 0o600, OLD_NPM_SHIM)
        .program(r"C:\npm\node_modules\@anthropic-ai\claude-code\cli.js")
        .program(r"C:\npm\node.exe")
        .program(r"C:\Program Files\nodejs\node.exe");
    assert_eq!(
        shim_launch(r"C:\npm\claude.cmd", &files, r"C:\Program Files\nodejs"),
        Launch {
            program: r"C:\npm\node.exe".into(),
            args: vec![r"C:\npm\node_modules\@anthropic-ai\claude-code\cli.js".into()],
        }
    );
}

#[test]
fn a_shim_for_a_native_program_starts_it_directly() {
    let files = Files::windows()
        .with(r"C:\npm\tool.cmd", 0o600, EXE_SHIM)
        .program(r"C:\npm\node_modules\tool\bin\tool.exe");
    assert_eq!(
        shim_launch(r"C:\npm\tool.cmd", &files, ""),
        Launch::direct(r"C:\npm\node_modules\tool\bin\tool.exe")
    );
}

#[test]
fn a_pnpm_shim_is_read_too() {
    let files = Files::windows()
        .with(r"C:\pnpm\bin\cli.CMD", 0o600, PNPM_SHIM)
        // The `..` stays for Windows to resolve, so the fake files name it.
        .program(r"C:\pnpm\bin\..\pkg\bin\cli.js")
        .program(r"C:\node\node.exe");
    assert_eq!(
        shim_launch(r"C:\pnpm\bin\cli.CMD", &files, r"C:\node"),
        Launch {
            program: r"C:\node\node.exe".into(),
            args: vec![r"C:\pnpm\bin\..\pkg\bin\cli.js".into()],
        }
    );
}

#[test]
fn a_shim_starts_as_it_is_when_it_cant_be_looked_through() {
    let plain = "@echo off\r\necho hello %*\r\n";
    let files = Files::windows()
        .with(r"C:\bin\plain.cmd", 0o600, plain)
        .with(r"C:\npm\nonode.cmd", 0o600, NPM_SHIM)
        .program(r"C:\npm\node_modules\@openai\codex\bin\codex.js")
        .program(r"C:\bin\tool.exe");
    // A batch file that isn't a shim.
    assert_eq!(
        shim_launch(r"C:\bin\plain.cmd", &files, ""),
        Launch::direct(r"C:\bin\plain.cmd")
    );
    // A shim whose script is gone.
    let gone = Files::windows().with(r"C:\npm\gone.cmd", 0o600, NPM_SHIM);
    assert_eq!(
        shim_launch(r"C:\npm\gone.cmd", &gone, ""),
        Launch::direct(r"C:\npm\gone.cmd")
    );
    // A shim with no `node.exe` anywhere.
    assert_eq!(
        shim_launch(r"C:\npm\nonode.cmd", &files, r"C:\bin"),
        Launch::direct(r"C:\npm\nonode.cmd")
    );
    // A program that isn't a batch file.
    assert_eq!(
        shim_launch(r"C:\bin\tool.exe", &files, ""),
        Launch::direct(r"C:\bin\tool.exe")
    );
}

#[test]
fn launch_builds_a_command_with_the_shims_arguments_first() {
    let launch = Launch {
        program: "node".into(),
        args: vec!["cli.js".into()],
    };
    let command = launch.command();
    assert_eq!(command.get_program(), "node");
    assert_eq!(command.get_args().collect::<Vec<_>>(), ["cli.js"]);
}

// The host.

#[test]
fn the_host_finds_a_program_everyone_has() {
    let name = if cfg!(windows) { "cmd" } else { "sh" };
    let found = find(name, &[]).expect("a shell on PATH");
    assert!(runnable(&found), "{}", found.display());
    assert!(!runnable(&std::env::temp_dir()), "a folder is no program");
}
