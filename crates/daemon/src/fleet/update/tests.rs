// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::*;
use crate::testing::TempDir;

/// A script that prints the first 16 hex characters of its own sha256,
/// as `maxd --build` does. `tag` makes each one a different build.
fn honest(tag: &str) -> String {
    format!(
        "#!/bin/sh\n# {tag}\nif command -v sha256sum >/dev/null; then sum=$(sha256sum \"$0\"); \
         else sum=$(shasum -a 256 \"$0\"); fi\nprintf '%s\\n' \"$sum\" | cut -c1-16\n"
    )
}

/// Writes an executable script.
fn script(path: &Path, text: &str) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    path.to_owned()
}

/// A home with `old` installed the way the first update finds it: a plain
/// file at `~/.local/bin/maxd`.
fn home_with(old: &str) -> (TempDir, Layout) {
    let dir = TempDir::new();
    let layout = Layout::new(dir.path());
    script(&layout.link(), old);
    (dir, layout)
}

/// Stages `text` as an upload and gives its path and sha256.
fn stage(dir: &TempDir, text: &str) -> (PathBuf, String) {
    let path = script(&dir.path().join("staged/maxd"), text);
    let sha = identity::sha256_of(&path).unwrap();
    (path, sha)
}

#[tokio::test]
async fn an_upload_with_the_wrong_checksum_is_refused() {
    let (dir, layout) = home_with(&honest("old"));
    let (staged, _) = stage(&dir, &honest("new"));
    let wrong = "0".repeat(64);
    let result = install_maxd(&layout, &staged, &wrong, &layout.link()).await;
    assert_eq!(result, Err(Refusal::Checksum));
    assert!(!layout.versions().exists());
}

#[tokio::test]
async fn a_binary_that_reports_another_build_is_refused() {
    let (dir, layout) = home_with(&honest("old"));
    let (staged, sha) = stage(&dir, "#!/bin/sh\necho 0123456789abcdef\n");
    let result = install_maxd(&layout, &staged, &sha, &layout.link()).await;
    assert!(matches!(result, Err(Refusal::Build(_))), "{result:?}");
    // Nothing was flipped and nothing was left behind.
    assert!(!fs::symlink_metadata(layout.link())
        .unwrap()
        .file_type()
        .is_symlink());
    let left: Vec<_> = fs::read_dir(layout.versions()).unwrap().flatten().collect();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
async fn a_binary_that_hangs_is_refused() {
    let dir = TempDir::new();
    let binary = script(&dir.path().join("slow"), "#!/bin/sh\nsleep 5\n");
    let result = check_build(&binary, Duration::from_millis(200)).await;
    assert_eq!(result, Err(Refusal::Build("it took too long".to_owned())));
}

#[tokio::test]
async fn a_maxd_that_isnt_linked_is_refused() {
    let (dir, layout) = home_with(&honest("old"));
    let elsewhere = script(&dir.path().join("dev/maxd"), &honest("dev"));
    let (staged, sha) = stage(&dir, &honest("new"));
    let result = install_maxd(&layout, &staged, &sha, &elsewhere).await;
    assert_eq!(result, Err(Refusal::NotLinked));
}

#[tokio::test]
async fn an_update_flips_the_link_and_keeps_the_previous_version() {
    let (dir, layout) = home_with(&honest("old"));
    let old_build = identity::build_of(&layout.link()).unwrap();
    let (staged, sha) = stage(&dir, &honest("new"));
    let new_build = identity::build_of(&staged).unwrap();

    let installed = install_maxd(&layout, &staged, &sha, &layout.link())
        .await
        .unwrap();

    assert_eq!(
        installed,
        Installed::Flipped {
            build: new_build.clone(),
            previous: old_build.clone(),
        }
    );
    let link = layout.link();
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(&link).unwrap(), layout.version(&new_build));
    assert!(layout.version(&old_build).exists());
    let state = UpdateState::read(&layout).unwrap();
    assert_eq!(state.build, new_build);
    assert_eq!(state.previous.as_deref(), Some(old_build.as_str()));
    assert!(!state.healthy);

    // A second update keeps only the one before it.
    let (staged, sha) = stage(&dir, &honest("newer"));
    let newer = identity::build_of(&staged).unwrap();
    install_maxd(&layout, &staged, &sha, &link).await.unwrap();
    assert!(layout.version(&newer).exists());
    assert!(layout.version(&new_build).exists());
    assert!(!layout.version(&old_build).exists());
}

#[tokio::test]
async fn the_running_build_again_changes_nothing() {
    let (dir, layout) = home_with(&honest("same"));
    let text = fs::read_to_string(layout.link()).unwrap();
    let (staged, sha) = stage(&dir, &text);
    let result = install_maxd(&layout, &staged, &sha, &layout.link()).await;
    assert_eq!(result, Ok(Installed::Same));
    assert!(UpdateState::read(&layout).is_none());
}

#[tokio::test]
async fn a_guest_binary_is_replaced_whole() {
    let dir = TempDir::new();
    let guest = script(&dir.path().join("lib/maxd-guest"), &honest("old guest"));
    let (staged, sha) = stage(&dir, &honest("new guest"));
    let build = install_guest(&staged, &sha, &guest).await.unwrap();
    assert_eq!(identity::build_of(&guest).unwrap(), build);
    let mode = fs::metadata(&guest).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
}

/// Counts restarts instead of making them.
#[derive(Default)]
struct FakeServices {
    restarts: u32,
}

impl Services for FakeServices {
    fn restart(&mut self) -> io::Result<()> {
        self.restarts += 1;
        Ok(())
    }
}

/// A home just after an update from `old` to `new`.
async fn updated() -> (TempDir, Layout, String, String) {
    let (dir, layout) = home_with(&honest("old"));
    let (staged, sha) = stage(&dir, &honest("new"));
    let Installed::Flipped { build, previous } =
        install_maxd(&layout, &staged, &sha, &layout.link())
            .await
            .unwrap()
    else {
        panic!("the update changed nothing");
    };
    (dir, layout, build, previous)
}

#[tokio::test]
async fn the_watcher_leaves_a_healthy_update_alone() {
    let (_dir, layout, build, _) = updated().await;
    assert!(awaiting_health(&layout, &build));
    assert!(mark_healthy(&layout, &build));
    assert!(!mark_healthy(&layout, &build));
    let mut services = FakeServices::default();
    let watched = watch(
        &layout,
        &build,
        Duration::from_secs(5),
        Duration::from_millis(10),
        &mut services,
    )
    .unwrap();
    assert_eq!(watched, Watched::Healthy);
    assert_eq!(services.restarts, 0);
    assert_eq!(
        fs::read_link(layout.link()).unwrap(),
        layout.version(&build)
    );
}

#[tokio::test]
async fn the_watcher_puts_the_old_build_back_when_the_new_one_never_comes_up() {
    let (_dir, layout, build, previous) = updated().await;
    let mut services = FakeServices::default();
    let watched = watch(
        &layout,
        &build,
        Duration::from_millis(100),
        Duration::from_millis(10),
        &mut services,
    )
    .unwrap();
    assert_eq!(
        watched,
        Watched::RolledBack {
            previous: previous.clone()
        }
    );
    assert_eq!(services.restarts, 1);
    assert_eq!(
        fs::read_link(layout.link()).unwrap(),
        layout.version(&previous)
    );
    let state = UpdateState::read(&layout).unwrap();
    assert_eq!(state.build, previous);
    assert!(state.healthy);
}

#[tokio::test]
async fn the_watcher_stops_when_another_update_replaces_its_own() {
    let (_dir, layout, build, previous) = updated().await;
    UpdateState {
        previous: Some(build.clone()),
        build: "fedcba9876543210".to_owned(),
        healthy: false,
        at: 0,
    }
    .write(&layout)
    .unwrap();
    let mut services = FakeServices::default();
    let watched = watch(
        &layout,
        &build,
        Duration::from_secs(5),
        Duration::from_millis(10),
        &mut services,
    )
    .unwrap();
    assert_eq!(watched, Watched::Superseded);
    assert_eq!(services.restarts, 0);
    assert!(layout.version(&previous).exists());
}

#[tokio::test]
async fn the_watcher_leaves_a_maxd_put_there_by_something_else() {
    let (_dir, layout, build, previous) = updated().await;
    let mut services = FakeServices::default();
    let link = layout.link();
    let replace = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        fs::remove_file(&link).unwrap();
        script(&link, &honest("deployed"));
    });
    let watched = watch(
        &layout,
        &build,
        Duration::from_secs(2),
        Duration::from_millis(10),
        &mut services,
    )
    .unwrap();
    replace.join().unwrap();
    assert_eq!(watched, Watched::Superseded);
    assert_eq!(services.restarts, 0);
    assert!(!fs::symlink_metadata(layout.link()).unwrap().is_symlink());
    assert_eq!(UpdateState::read(&layout).unwrap().build, build);
    assert!(layout.version(&previous).exists());
}

/// Records what would run.
#[derive(Default)]
struct Recorded(Vec<String>);

impl Runner for Recorded {
    fn run(&mut self, program: &str, args: &[&str]) -> io::Result<service::Ran> {
        self.0.push(format!("{program} {}", args.join(" ")));
        Ok(service::Ran {
            success: true,
            ..Default::default()
        })
    }
}

#[test]
fn a_restart_asks_the_service_manager() {
    let mut launchd = SystemServices {
        manager: Manager::Launchd,
        uid: 501,
        runner: Recorded::default(),
    };
    launchd.restart().unwrap();
    assert_eq!(
        launchd.runner.0,
        ["launchctl kickstart -k gui/501/com.maxagenthq.maxd"]
    );
    let mut systemd = SystemServices {
        manager: Manager::Systemd,
        uid: 1000,
        runner: Recorded::default(),
    };
    systemd.restart().unwrap();
    assert_eq!(systemd.runner.0, ["systemctl --user restart maxd.service"]);
}

/// A minimal 64-bit ELF header with one program header of `kind`.
fn elf(kind: u32) -> Vec<u8> {
    let mut bytes = vec![0u8; 64 + 56];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
    bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&1u16.to_le_bytes());
    bytes[64..68].copy_from_slice(&kind.to_le_bytes());
    bytes
}

#[test]
fn only_a_static_linux_build_counts_as_a_guest() {
    let dir = TempDir::new();
    let static_build = dir.path().join("static");
    fs::write(&static_build, elf(1)).unwrap();
    let dynamic = dir.path().join("dynamic");
    fs::write(&dynamic, elf(3)).unwrap();
    let shell = script(&dir.path().join("shell"), &honest("shell"));
    assert!(is_static_elf(&static_build));
    assert!(!is_static_elf(&dynamic));
    assert!(!is_static_elf(&shell));
    assert!(!is_static_elf(&dir.path().join("missing")));
}
