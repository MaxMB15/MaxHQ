// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The logon task's command lines and the stop request, against a fake
//! `schtasks` and fake processes, so they run on any system.

use std::fs;
use std::path::PathBuf;

use maxhq_core::service::{program_in_task, task_file_text, task_name};

use super::super::tests::{setup, Fake};
use super::super::Manager;
use super::*;
use crate::testing::TempDir;

const SID: &str = "S-1-5-21-1-2-3-1001";

/// A `maxd` that serves, or doesn't, and how it takes being stopped.
#[derive(Default)]
struct Procs {
    /// The process ID of the `maxd` that serves.
    maxd: Option<u32>,
    /// Whether it stops when it's asked to.
    obeys: bool,
    /// Whether it ends when it's killed.
    killable: bool,
    /// What was done to it, in order.
    did: Vec<String>,
}

impl Procs {
    fn serving(pid: u32) -> Self {
        Self {
            maxd: Some(pid),
            obeys: true,
            killable: true,
            ..Self::default()
        }
    }
}

impl Processes for Procs {
    fn ask_stop(&mut self, _socket: &Path) -> io::Result<u32> {
        let pid = self.maxd.ok_or(io::ErrorKind::NotFound)?;
        self.did.push(format!("ask {pid}"));
        if self.obeys {
            self.maxd = None;
        }
        Ok(pid)
    }

    fn serving(&mut self, _socket: &Path) -> Option<u32> {
        self.maxd
    }

    fn alive(&mut self, pid: u32) -> bool {
        self.maxd == Some(pid)
    }

    fn kill(&mut self, pid: u32) -> io::Result<()> {
        self.did.push(format!("kill {pid}"));
        if self.killable {
            self.maxd = None;
        }
        Ok(())
    }
}

/// A console program's first bytes, as far as [`launcher::windowless`]
/// reads them.
fn console_program() -> Vec<u8> {
    let header = 0x80;
    let mut image = vec![0; 0x200];
    image[..2].copy_from_slice(b"MZ");
    image[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    image[header..header + 4].copy_from_slice(b"PE\0\0");
    image[header + 24..header + 26].copy_from_slice(&0x20bu16.to_le_bytes());
    image[header + 92..header + 94].copy_from_slice(&3u16.to_le_bytes());
    image
}

/// A Windows setup under `dir`, with MaxHQ's folder, the data inside it
/// and a `maxd.exe` there.
fn windows(dir: &TempDir) -> Setup {
    let mut setup = setup(dir, Manager::TaskScheduler);
    let app = dir.path().join("MaxHQ");
    setup.data = Some(app.join("data"));
    setup.runtime = setup.data.clone();
    setup.app = Some(app.clone());
    setup.exe = app.join("versions").join("1").join("maxd.exe");
    fs::create_dir_all(setup.exe.parent().unwrap()).unwrap();
    fs::write(&setup.exe, console_program()).unwrap();
    setup
}

fn name() -> String {
    task_name(SID)
}

#[test]
fn install_writes_the_launcher_and_the_task_and_runs_it() {
    let dir = TempDir::new();
    let setup = windows(&dir);
    let mut runner = Fake::default();
    let mut procs = Procs::default();
    let report = install_as(&setup, SID, &mut runner, &mut procs, Duration::ZERO).unwrap();

    let file = task_file(&setup.app_folder());
    let name = name();
    assert_eq!(
        runner.ran,
        [
            format!("schtasks /End /TN {name}"),
            format!("schtasks /Create /TN {name} /XML {} /F", file.display()),
            format!("schtasks /Run /TN {name}"),
        ]
    );
    let launcher = fs::read(launcher_program(&setup.app_folder())).unwrap();
    let mut windowless = console_program();
    launcher::windowless(&mut windowless).unwrap();
    assert_eq!(launcher, windowless);

    let text = task_file_text(&fs::read(&file).unwrap()).unwrap();
    assert_eq!(text, setup.task_contents(SID));
    assert!(text.contains(&format!("<UserId>{SID}</UserId>")));
    assert_eq!(program_in_task(&text), Some(setup.exe.clone()));
    assert!(report[0].contains(&name), "{report:?}");
    assert!(
        report[1].contains(&setup.log_file().display().to_string()),
        "{report:?}"
    );
}

#[test]
fn install_stops_the_maxd_that_runs_first() {
    let dir = TempDir::new();
    let setup = windows(&dir);
    let mut runner = Fake::default();
    let mut procs = Procs::serving(42);
    install_as(&setup, SID, &mut runner, &mut procs, Duration::ZERO).unwrap();
    assert_eq!(procs.did, ["ask 42"]);
    assert_eq!(runner.ran[0], format!("schtasks /End /TN {}", name()));
}

#[test]
fn install_refuses_what_isnt_a_windows_program() {
    let dir = TempDir::new();
    let setup = windows(&dir);
    fs::write(&setup.exe, b"#!/bin/sh\n").unwrap();
    let mut runner = Fake::default();
    let err = install_as(
        &setup,
        SID,
        &mut runner,
        &mut Procs::default(),
        Duration::ZERO,
    )
    .unwrap_err();
    assert!(matches!(err, ServiceError::Io { .. }), "{err}");
    assert!(!runner.ran.iter().any(|line| line.contains("/Create")));
}

#[test]
fn a_failed_registration_says_so() {
    let dir = TempDir::new();
    let setup = windows(&dir);
    let mut runner = Fake::default().fail("schtasks /Create", "access denied");
    let err = install_as(
        &setup,
        SID,
        &mut runner,
        &mut Procs::default(),
        Duration::ZERO,
    )
    .unwrap_err();
    assert!(err.to_string().contains("access denied"), "{err}");
    assert!(!runner.ran.iter().any(|line| line.contains("/Run")));
}

#[test]
fn stop_asks_maxd_then_ends_the_task() {
    let dir = TempDir::new();
    let setup = windows(&dir);
    let mut runner = Fake::default();
    let mut procs = Procs::serving(42);
    assert!(stop_as(&setup, &name(), &mut runner, &mut procs, Duration::ZERO).unwrap());
    assert_eq!(procs.did, ["ask 42"]);
    assert_eq!(runner.ran, [format!("schtasks /End /TN {}", name())]);
}

#[test]
fn a_maxd_that_doesnt_stop_when_asked_is_killed() {
    let dir = TempDir::new();
    let setup = windows(&dir);
    let mut runner = Fake::default();
    let mut procs = Procs {
        obeys: false,
        ..Procs::serving(42)
    };
    assert!(stop_as(&setup, &name(), &mut runner, &mut procs, Duration::ZERO).unwrap());
    assert_eq!(procs.did, ["ask 42", "kill 42"]);
}

#[test]
fn a_maxd_that_wont_take_the_request_is_killed_after_the_task_ends() {
    let dir = TempDir::new();
    let setup = windows(&dir);
    let mut runner = Fake::default();
    // One from before the stop endpoint, say.
    struct Deaf(Procs);
    impl Processes for Deaf {
        fn ask_stop(&mut self, _: &Path) -> io::Result<u32> {
            Err(io::ErrorKind::TimedOut.into())
        }
        fn serving(&mut self, socket: &Path) -> Option<u32> {
            self.0.serving(socket)
        }
        fn alive(&mut self, pid: u32) -> bool {
            self.0.alive(pid)
        }
        fn kill(&mut self, pid: u32) -> io::Result<()> {
            self.0.kill(pid)
        }
    }
    let mut procs = Deaf(Procs::serving(42));
    assert!(stop_as(&setup, &name(), &mut runner, &mut procs, Duration::ZERO).unwrap());
    assert_eq!(procs.0.did, ["kill 42"]);
    assert_eq!(runner.ran, [format!("schtasks /End /TN {}", name())]);
}

#[test]
fn a_maxd_that_cant_be_killed_fails_the_stop() {
    let dir = TempDir::new();
    let setup = windows(&dir);
    let mut runner = Fake::default();
    let mut procs = Procs {
        obeys: false,
        killable: false,
        ..Procs::serving(42)
    };
    let err = stop_as(&setup, &name(), &mut runner, &mut procs, Duration::ZERO).unwrap_err();
    assert!(err.to_string().contains("process 42"), "{err}");
}

#[test]
fn stopping_what_doesnt_run_says_so() {
    let dir = TempDir::new();
    let setup = windows(&dir);
    // Ending a task that doesn't run fails, which is fine.
    let mut runner = Fake::default().fail("schtasks /End", "not running");
    let mut procs = Procs::default();
    assert!(!stop_as(&setup, &name(), &mut runner, &mut procs, Duration::ZERO).unwrap());
    assert!(procs.did.is_empty());
}

#[test]
fn run_needs_the_task() {
    let dir = TempDir::new();
    let setup = windows(&dir);
    let mut runner = Fake::default().fail("schtasks /Query", "no such task");
    let err = run_named(&setup, &name(), &mut runner).unwrap_err();
    assert!(
        matches!(&err, ServiceError::NotInstalled(file) if *file == setup.file()),
        "{err}"
    );
    assert!(!runner.ran.iter().any(|line| line.contains("/Run")));

    let mut runner = Fake::default();
    run_named(&setup, &name(), &mut runner).unwrap();
    assert_eq!(
        runner.ran,
        [
            format!("schtasks /Query /TN {}", name()),
            format!("schtasks /Run /TN {}", name()),
        ]
    );
}

#[test]
fn start_leaves_a_maxd_that_serves_alone() {
    let dir = TempDir::new();
    let setup = windows(&dir);
    let mut runner = Fake::default();
    assert!(!start_named(&setup, &name(), &mut runner, &mut Procs::serving(42)).unwrap());
    assert!(!runner.ran.iter().any(|line| line.contains("/Run")));

    let mut runner = Fake::default();
    assert!(start_named(&setup, &name(), &mut runner, &mut Procs::default()).unwrap());
    assert_eq!(
        runner.ran.last().unwrap(),
        &format!("schtasks /Run /TN {}", name())
    );

    let mut runner = Fake::default().fail("schtasks /Query", "no such task");
    let err = start_named(&setup, &name(), &mut runner, &mut Procs::default()).unwrap_err();
    assert!(matches!(err, ServiceError::NotInstalled(_)), "{err}");
}

#[test]
fn state_reads_the_task_and_the_endpoint() {
    let dir = TempDir::new();
    let setup = windows(&dir);
    let mut gone = Fake::default().fail("schtasks /Query", "no such task");
    assert_eq!(
        state_of(&setup, &name(), &mut gone, &mut Procs::serving(42)),
        ("not loaded".to_owned(), None)
    );
    assert_eq!(
        state_of(
            &setup,
            &name(),
            &mut Fake::default(),
            &mut Procs::serving(42)
        ),
        ("running".to_owned(), Some(42))
    );
    assert_eq!(
        state_of(&setup, &name(), &mut Fake::default(), &mut Procs::default()),
        ("stopped".to_owned(), None)
    );
}

#[test]
fn the_windows_setup_keeps_the_task_beside_the_data_and_the_log_in_it() {
    let dir = TempDir::new();
    let setup = windows(&dir);
    let app = setup.app_folder();
    assert_eq!(setup.file(), app.join("maxd-task.xml"));
    let data = setup.data_folder();
    assert_eq!(data, app.join("data"));
    assert_eq!(setup.log_file(), data.join("Logs").join("maxd.log"));
    let contents = setup.task_contents(SID);
    assert!(contents.contains(&format!(
        "<Command>{}</Command>",
        launcher_program(&app).display()
    )));

    // Without the folders, they're where Windows has them.
    let mut bare = setup.clone();
    bare.app = None;
    bare.data = None;
    let local = setup.home.join("AppData").join("Local").join("MaxHQ");
    assert_eq!(bare.app_folder(), local);
    assert_eq!(bare.data_folder(), local.join("data"));
}

#[test]
fn a_verbatim_drive_path_reads_plainly() {
    use super::super::plain;
    assert_eq!(
        plain(PathBuf::from(r"\\?\C:\Users\me\maxd.exe")),
        PathBuf::from(r"C:\Users\me\maxd.exe")
    );
    // A share's verbatim path has no plain form of the same kind.
    assert_eq!(
        plain(PathBuf::from(r"\\?\UNC\server\share\maxd.exe")),
        PathBuf::from(r"\\?\UNC\server\share\maxd.exe")
    );
    assert_eq!(
        plain(PathBuf::from("/usr/bin/maxd")),
        PathBuf::from("/usr/bin/maxd")
    );
}
