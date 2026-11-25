// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::collections::HashMap;

use super::*;

/// Files by their Windows path: a mode and, for a shim, its text.
#[derive(Default)]
struct Files(HashMap<String, Option<String>>);

impl Files {
    fn with(mut self, path: &str) -> Self {
        self.0.insert(path.to_owned(), None);
        self
    }

    fn shim(mut self, path: &str, text: &str) -> Self {
        self.0.insert(path.to_owned(), Some(text.to_owned()));
        self
    }
}

impl Fs for Files {
    fn file_mode(&self, path: &Path) -> Option<u32> {
        self.0.get(&*path.to_string_lossy()).map(|_| 0o644)
    }

    fn read_text(&self, path: &Path) -> Option<String> {
        self.0.get(&*path.to_string_lossy()).cloned().flatten()
    }
}

fn folders() -> WindowsFolders {
    WindowsFolders {
        home: Some(PathBuf::from(r"C:\Users\me")),
        app_data: Some(PathBuf::from(r"C:\Users\me\AppData\Roaming")),
        local_app_data: Some(PathBuf::from(r"C:\Users\me\AppData\Local")),
    }
}

fn texts(dirs: &[PathBuf]) -> Vec<String> {
    dirs.iter()
        .map(|dir| dir.to_string_lossy().into_owned())
        .collect()
}

fn args(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

#[test]
fn each_cli_has_its_windows_install_folders_first() {
    let shared = [
        r"C:\Users\me\AppData\Roaming\npm",
        r"C:\Users\me\AppData\Local\pnpm",
        r"C:\Users\me\AppData\Local\Volta\bin",
        r"C:\Users\me\.bun\bin",
        r"C:\Users\me\scoop\shims",
        r"C:\Users\me\AppData\Local\Microsoft\WinGet\Links",
    ];
    let claude = texts(&folders().of(Cli::Claude));
    assert_eq!(
        claude[..2],
        [r"C:\Users\me\.local\bin", r"C:\Users\me\.claude\local"]
    );
    assert_eq!(claude[2..], shared);
    let cursor = texts(&folders().of(Cli::Cursor));
    assert_eq!(cursor[0], r"C:\Users\me\AppData\Local\cursor-agent");
    assert_eq!(cursor[2..], shared);
    let codex = texts(&folders().of(Cli::Codex));
    assert_eq!(codex[0], r"C:\Users\me\.local\bin");
    assert_eq!(codex[1..], shared);
}

#[test]
fn folders_windows_didnt_give_are_left_out() {
    let only_home = WindowsFolders {
        home: Some(PathBuf::from(r"C:\Users\me\")),
        ..WindowsFolders::default()
    };
    assert_eq!(
        texts(&only_home.of(Cli::Codex)),
        [
            r"C:\Users\me\.local\bin",
            r"C:\Users\me\.bun\bin",
            r"C:\Users\me\scoop\shims"
        ]
    );
    assert!(WindowsFolders::default().of(Cli::Claude).is_empty());
}

#[test]
fn windows_finds_the_exe_before_the_npm_shim() {
    let files = Files::default()
        .with(r"C:\Users\me\AppData\Roaming\npm\codex.cmd")
        .with(r"C:\Users\me\AppData\Roaming\npm\codex.exe")
        .with(r"C:\tools\codex");
    let path = OsString::from(r"C:\tools;C:\Users\me\AppData\Roaming\npm");
    // `codex` without an extension isn't a program on Windows.
    assert_eq!(
        find_in(Rules::Windows, "codex", Some(&path), &[], &files),
        Some(PathBuf::from(r"C:\Users\me\AppData\Roaming\npm\codex.exe"))
    );
}

#[test]
fn windows_looks_in_the_install_folders_after_path() {
    let files = Files::default().with(r"C:\Users\me\AppData\Local\cursor-agent\agent.exe");
    let path = OsString::from(r"C:\Windows\System32");
    let extra = folders().of(Cli::Cursor);
    assert_eq!(
        find_in(Rules::Windows, "cursor-agent", Some(&path), &extra, &files),
        None
    );
    assert_eq!(
        find_in(Rules::Windows, "agent", Some(&path), &extra, &files),
        Some(PathBuf::from(
            r"C:\Users\me\AppData\Local\cursor-agent\agent.exe"
        ))
    );
    // A relative folder on PATH never counts.
    let relative = OsString::from(r"bin;.\tools");
    let files = files.with(r"bin\agent.exe");
    assert_eq!(
        find_in(Rules::Windows, "agent", Some(&relative), &[], &files),
        None
    );
}

#[test]
fn an_npm_shim_starts_the_script_through_node() {
    let shim = r"C:\Users\me\AppData\Roaming\npm\codex.cmd";
    let script = r"C:\Users\me\AppData\Roaming\npm\node_modules\@openai\codex\bin\codex.js";
    let files = Files::default()
        .shim(
            shim,
            "@ECHO off\r\nGOTO start\r\n:start\r\nSETLOCAL\r\n\
             \"%_prog%\"  \"%dp0%\\node_modules\\@openai\\codex\\bin\\codex.js\" %*\r\n",
        )
        .with(script)
        .with(r"C:\Users\me\AppData\Roaming\npm\node.exe");
    let env = Env::default();
    let found = which::launch_in(Rules::Windows, Path::new(shim), &env, &files);
    let mut program = PathBuf::from(shim);
    let mut started = args(&["app-server", "-c", "model=\"gpt\""]);
    resolve_to(&mut program, &mut started, found);
    assert_eq!(
        program,
        PathBuf::from(r"C:\Users\me\AppData\Roaming\npm\node.exe")
    );
    assert_eq!(
        started,
        [script, "app-server", "-c", "model=\"gpt\""].map(OsString::from)
    );
    // The script runs without cmd.exe, so nothing is refused.
    assert_eq!(batch_refusal_in(Rules::Windows, &program, &started), None);
}

#[test]
fn a_program_that_starts_as_it_is_stays_put() {
    let claude = PathBuf::from("/home/me/.local/bin/claude");
    let (mut program, mut started) = (claude.clone(), args(&["-p"]));
    resolve_to(&mut program, &mut started, which::Launch::direct(&claude));
    assert_eq!((&program, &started[..]), (&claude, &args(&["-p"])[..]));
}

#[test]
#[cfg(unix)]
fn unix_readies_every_program_as_it_is() {
    let dir = crate::testing::TempDir::new();
    let shim = dir.path().join("agent.cmd");
    std::fs::write(&shim, "\"%dp0%\\agent.exe\" %*\r\n").unwrap();
    let (mut program, mut started) = (shim.clone(), args(&["two\nlines"]));
    assert_eq!(ready(&mut program, &mut started), Ok(()));
    assert_eq!((program, started), (shim, args(&["two\nlines"])));
}

#[test]
fn a_batch_file_takes_arguments_cmd_can_quote() {
    let program = Path::new(r"C:\Users\me\AppData\Local\tool\agent.CMD");
    let fine = args(&["acp", "--mode", "plan", r#"{"a":"50%"}"#, "a & b | c"]);
    assert_eq!(batch_refusal_in(Rules::Windows, program, &fine), None);
}

#[test]
fn a_batch_file_with_a_line_break_in_an_argument_is_refused() {
    for program in [r"C:\tools\agent.cmd", r"C:\tools\agent.bat"] {
        for arg in ["two\nlines", "carriage\rreturn", "nul\0byte"] {
            let reason = batch_refusal_in(Rules::Windows, Path::new(program), &args(&[arg]))
                .expect("refused");
            assert!(reason.contains("line break"), "{reason}");
            assert!(reason.starts_with("agent."), "{reason}");
        }
    }
}

#[test]
fn a_batch_command_line_past_what_cmd_reads_is_refused() {
    let program = Path::new(r"C:\tools\agent.cmd");
    let near = "x".repeat(8100);
    assert_eq!(
        batch_refusal_in(Rules::Windows, program, &args(&[&near])),
        None
    );
    // Each `%` costs eight characters once quoted.
    let percents = "%".repeat(1100);
    let reason = batch_refusal_in(Rules::Windows, program, &args(&[&percents])).expect("refused");
    assert!(reason.contains("longer than cmd.exe reads"), "{reason}");
}

#[test]
fn only_batch_files_on_windows_are_checked() {
    let long = "%".repeat(5000);
    let broken = args(&["two\nlines", &long]);
    for program in [
        r"C:\tools\agent.exe",
        r"C:\tools\agent",
        r"C:\tools.cmd\agent.exe",
    ] {
        assert_eq!(
            batch_refusal_in(Rules::Windows, Path::new(program), &broken),
            None
        );
    }
    assert_eq!(
        batch_refusal_in(Rules::Unix, Path::new("/usr/local/bin/agent.cmd"), &broken),
        None
    );
}

#[test]
fn path_puts_the_programs_folder_first_with_the_systems_separator() {
    assert_eq!(
        path_with_first_in(
            Rules::Unix,
            Path::new("/opt/bin"),
            Some(OsStr::new("/usr/bin"))
        ),
        "/opt/bin:/usr/bin"
    );
    assert_eq!(
        path_with_first_in(
            Rules::Windows,
            Path::new(r"C:\npm"),
            Some(OsStr::new(r"C:\Windows"))
        ),
        r"C:\npm;C:\Windows"
    );
    assert_eq!(
        path_with_first_in(Rules::Windows, Path::new(r"C:\npm"), Some(OsStr::new(""))),
        r"C:\npm"
    );
    assert_eq!(
        path_with_first_in(Rules::Unix, Path::new("/opt/bin"), None),
        "/opt/bin"
    );
}
