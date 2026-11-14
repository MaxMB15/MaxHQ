// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Runs `scripts/dev-app.sh`, Cargo's runner for `pnpm dev`, against fake
//! binaries: shell scripts that write where they ran from and with what.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn runner() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/dev-app.sh")
}

/// A fresh folder standing in for Cargo's target folder.
fn target(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("maxhq-dev-runner-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes a binary called `name` into `dir` that records its own path and
/// arguments, then prints `word`.
fn fake(dir: &Path, name: &str, word: &str) -> PathBuf {
    let path = dir.join(name);
    let ran = dir.join("ran");
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$0\" \"$@\" > '{}'\necho {word}\n",
            ran.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Runs the runner on `binary`, and gives what the binary printed and the
/// lines it recorded.
fn run(binary: &Path, args: &[&str]) -> (String, Vec<String>) {
    let output = Command::new("sh")
        .arg(runner())
        .arg(binary)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let ran = fs::read_to_string(binary.with_file_name("ran")).unwrap();
    (
        String::from_utf8(output.stdout).unwrap().trim().to_owned(),
        ran.lines().map(str::to_owned).collect(),
    )
}

#[test]
fn any_other_binary_runs_as_it_is() {
    let dir = target("other");
    let binary = fake(&dir, "maxhq_desktop-0123abcd", "tested");
    let (said, ran) = run(&binary, &["--exact", "a test", ""]);
    assert_eq!(said, "tested");
    assert_eq!(
        ran,
        [
            binary.display().to_string(),
            "--exact".into(),
            "a test".into(),
            String::new()
        ]
    );
    assert!(!dir.join("MaxHQ Dev.app").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn the_desktop_app_runs_from_its_dev_bundle() {
    let dir = target("app");
    let binary = fake(&dir, "maxhq-desktop", "first");
    let app = dir.join("MaxHQ Dev.app");
    let inside = app.join("Contents/MacOS/maxhq-desktop");

    let (said, ran) = run(&binary, &["--flag", "two words"]);
    assert_eq!(said, "first");
    assert_eq!(
        ran,
        [
            inside.display().to_string(),
            "--flag".into(),
            "two words".into()
        ]
    );

    let plist = fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
    for wanted in [
        "<string>com.maxagenthq.maxhq.dev</string>",
        "<string>maxhq-desktop</string>",
        "<string>MaxHQ Dev</string>",
        "<string>icon</string>",
    ] {
        assert!(plist.contains(wanted), "{wanted} is missing from {plist}");
    }
    assert!(app.join("Contents/Resources/icon.icns").is_file());
    // The app looks for maxd beside its own binary, and finds Cargo's.
    assert_eq!(
        fs::read_link(app.join("Contents/MacOS/maxd")).unwrap(),
        Path::new("../../../maxd")
    );

    // A rebuild runs the new binary, and Cargo's own file is left alone.
    let binary = fake(&dir, "maxhq-desktop", "second");
    let (said, _) = run(&binary, &[]);
    assert_eq!(said, "second");
    assert!(fs::read_to_string(&binary).unwrap().contains("second"));
    fs::remove_dir_all(dir).unwrap();
}
