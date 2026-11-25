// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use super::*;

/// The restart code the tests' plans use.
const RESTART: i32 = 75;

/// The filter that makes [`child`] run, with its marker file after it.
const CHILD: &str = "maxhq-launcher-child=";

/// The filter that sets [`child`]'s first exit code.
const FIRST_EXIT: &str = "maxhq-launcher-first-exit=";

/// The filter that makes [`tie_child`] run, with the file for the IDs it
/// starts after it.
#[cfg(windows)]
const TIE_CHILD: &str = "maxhq-launcher-tie=";

/// A folder no other test or run uses.
fn folder(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("maxhq-os-launcher-{test}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The value after `prefix` among this test binary's arguments.
fn arg(prefix: &str) -> Option<String> {
    std::env::args().find_map(|arg| arg.strip_prefix(prefix).map(str::to_owned))
}

fn plan(dir: &Path, args: Vec<String>) -> Plan {
    Plan {
        socket: dir.join("maxd.sock"),
        program: std::env::current_exe().unwrap(),
        args,
        log: dir.join("Logs").join("maxd.log"),
        log_limit: 1 << 20,
        restart_code: RESTART,
        backoff: Backoff {
            first: Duration::from_millis(10),
            max: Duration::from_millis(40),
            reset_after: Duration::from_secs(60),
        },
    }
}

/// Runs [`child`] with `marker`, whose first exit is `first`.
fn child_args(marker: &Path, first: i32) -> Vec<String> {
    vec![
        "--exact".to_owned(),
        "launcher::tests::child".to_owned(),
        "--nocapture".to_owned(),
        format!("{CHILD}{}", marker.display()),
        format!("{FIRST_EXIT}{first}"),
    ]
}

/// Not a test, but the program [`supervise`] runs in the tests, when
/// this binary starts with a [`CHILD`] filter. It prints a line, then
/// exits with its first code while the marker is missing, and with 0
/// once it's there.
#[test]
fn child() {
    let Some(marker) = arg(CHILD).map(PathBuf::from) else {
        return;
    };
    println!("the child ran");
    if marker.exists() {
        std::process::exit(0);
    }
    fs::write(&marker, "").unwrap();
    std::process::exit(
        arg(FIRST_EXIT)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0),
    );
}

#[test]
fn a_restart_starts_again_at_once_and_a_crash_after_a_pause() {
    let plan = plan(Path::new("unused"), Vec::new());
    let (pause, quick) = (Duration::from_millis(20), Duration::from_millis(5));
    assert_eq!(after(Some(0), quick, pause, &plan), Next::Stop);
    assert_eq!(after(Some(RESTART), quick, pause, &plan), Next::Again);
    assert_eq!(after(Some(1), quick, pause, &plan), Next::Wait(pause));
    assert_eq!(after(None, quick, pause, &plan), Next::Wait(pause));
    // A run that lasted starts the pauses over.
    let long = plan.backoff.reset_after;
    assert_eq!(
        after(Some(1), long, pause, &plan),
        Next::Wait(plan.backoff.first)
    );
}

#[test]
fn the_program_runs_until_it_exits_with_0() {
    let _forks = crate::forks();
    for first in [RESTART, 3] {
        let dir = folder(&format!("runs-{first}"));
        let marker = dir.join("marker");
        let plan = plan(&dir, child_args(&marker, first));
        supervise(&plan).unwrap();
        let log = fs::read_to_string(&plan.log).unwrap();
        assert_eq!(log.matches("the child ran").count(), 2, "{log}");
        // A crash is noted, and a restart isn't.
        assert_eq!(log.contains("exit code 3"), first == 3, "{log}");
        let _ = fs::remove_dir_all(&dir);
    }
}

#[test]
fn a_big_log_is_moved_aside_when_the_program_starts() {
    let _forks = crate::forks();
    let dir = folder("log");
    let marker = dir.join("marker");
    fs::write(&marker, "").unwrap();
    let mut plan = plan(&dir, child_args(&marker, 0));
    plan.log_limit = 10;
    fs::create_dir_all(plan.log.parent().unwrap()).unwrap();
    fs::write(&plan.log, "an old log, past the limit\n").unwrap();
    supervise(&plan).unwrap();
    let mut old = plan.log.as_os_str().to_owned();
    old.push(".1");
    assert_eq!(
        fs::read_to_string(PathBuf::from(old)).unwrap(),
        "an old log, past the limit\n"
    );
    assert!(fs::read_to_string(&plan.log)
        .unwrap()
        .contains("the child ran"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_maxd_that_serves_already_is_left_be() {
    let _forks = crate::forks();
    let dir = folder("serves");
    let mut plan = plan(&dir, Vec::new());
    // Never started.
    plan.program = dir.join("missing");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _listener = runtime
        .block_on(async { crate::ipc::LocalListener::bind(&plan.socket) })
        .unwrap();
    supervise(&plan).unwrap();
    let log = fs::read_to_string(&plan.log).unwrap();
    assert!(log.contains("another maxd serves already"), "{log}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_program_that_never_starts_is_given_up_on() {
    let _forks = crate::forks();
    let dir = folder("missing");
    let mut plan = plan(&dir, Vec::new());
    plan.program = dir.join("missing");
    plan.backoff.max = Duration::from_millis(10);
    let err = supervise(&plan).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
    let log = fs::read_to_string(&plan.log).unwrap();
    assert_eq!(log.matches("couldn't start").count(), START_TRIES as usize);
    let _ = fs::remove_dir_all(&dir);
}

/// A program's head: the DOS header pointing at the PE header at 0x80,
/// and an optional header of `magic` for `subsystem`.
fn image(magic: u16, subsystem: u16) -> Vec<u8> {
    let mut image = vec![0u8; 0x200];
    image[..2].copy_from_slice(b"MZ");
    image[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    image[0x80..0x84].copy_from_slice(b"PE\0\0");
    image[0x98..0x9a].copy_from_slice(&magic.to_le_bytes());
    image[0xdc..0xde].copy_from_slice(&subsystem.to_le_bytes());
    image
}

#[test]
fn a_console_program_becomes_windowed() {
    for magic in [0x10b, 0x20b] {
        let mut console = image(magic, CONSOLE);
        windowless(&mut console).unwrap();
        assert_eq!(console, image(magic, WINDOWED));
        // Once is enough.
        windowless(&mut console).unwrap();
        assert_eq!(console, image(magic, WINDOWED));
    }
}

#[test]
fn what_isnt_a_console_program_is_refused() {
    let native = image(0x20b, 1);
    let mut no_mz = image(0x20b, CONSOLE);
    no_mz[0] = b'X';
    let mut no_pe = image(0x20b, CONSOLE);
    no_pe[0x80] = b'X';
    let mut short = image(0x20b, CONSOLE);
    short.truncate(0xdd);
    let mut far = image(0x20b, CONSOLE);
    far[0x3c..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
    for (what, mut bad) in [
        ("native", native),
        ("no MZ", no_mz),
        ("no PE", no_pe),
        ("short", short),
        ("far", far),
    ] {
        let before = bad.clone();
        let err = windowless(&mut bad).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{what}");
        assert_eq!(bad, before, "{what}");
    }
}

/// The subsystem of the program at `path`.
#[cfg(windows)]
fn subsystem_of(path: &Path) -> u16 {
    let image = fs::read(path).unwrap();
    let header = u32::from_le_bytes(image[0x3c..0x40].try_into().unwrap()) as usize;
    let at = header + 24 + 68;
    u16::from_le_bytes([image[at], image[at + 1]])
}

#[cfg(windows)]
#[test]
fn a_windowless_copy_runs_without_a_console() {
    let _forks = crate::forks();
    let dir = folder("copy");
    let to = dir.join("maxdw.exe");
    let this = std::env::current_exe().unwrap();
    windowless_copy(&this, &to).unwrap();
    assert_eq!(subsystem_of(&this), CONSOLE);
    assert_eq!(subsystem_of(&to), WINDOWED);
    // A copy that's there already is kept.
    let written = fs::metadata(&to).unwrap().modified().unwrap();
    windowless_copy(&this, &to).unwrap();
    assert_eq!(fs::metadata(&to).unwrap().modified().unwrap(), written);
    // Started with its output taken, it writes as a console program does.
    let listed = Command::new(&to).arg("--list").output().unwrap();
    assert!(listed.status.success());
    assert!(String::from_utf8_lossy(&listed.stdout).contains("launcher::tests::child"));
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(windows)]
#[test]
fn a_running_copy_is_moved_aside() {
    let _forks = crate::forks();
    let dir = folder("running");
    let to = dir.join("maxdw.exe");
    fs::copy(r"C:\Windows\System32\PING.EXE", &to).unwrap();
    let mut running = Command::new(&to)
        .args(["-n", "60", "127.0.0.1"])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    windowless_copy(&std::env::current_exe().unwrap(), &to).unwrap();
    assert_eq!(subsystem_of(&to), WINDOWED);
    assert!(dir.join("maxdw.exe.old").exists());
    running.kill().unwrap();
    running.wait().unwrap();
    // The next copy clears it.
    windowless_copy(&std::env::current_exe().unwrap(), &to).unwrap();
    assert!(!dir.join("maxdw.exe.old").exists());
    let _ = fs::remove_dir_all(&dir);
}

/// Not a test, but a launcher in the tie test, when this binary starts
/// with a [`TIE_CHILD`] filter: it ties its children, starts one that
/// stays in its job and one that leaves, writes their IDs, and exits.
#[cfg(windows)]
#[test]
fn tie_child() {
    let Some(ids) = arg(TIE_CHILD).map(PathBuf::from) else {
        return;
    };
    tie_children().unwrap();
    let ping = || {
        let mut command = Command::new("ping");
        command
            .args(["-n", "60", "127.0.0.1"])
            .stdout(Stdio::null());
        command
    };
    let tied = ping().spawn().unwrap();
    let mut left = ping();
    left.creation_flags(windows_sys::Win32::System::Threading::CREATE_BREAKAWAY_FROM_JOB);
    // A job this one is in may not let it leave, which is 0 here.
    let left = match left.spawn() {
        Ok(left) => left.id(),
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => 0,
        Err(err) => panic!("{err}"),
    };
    fs::write(&ids, format!("{} {left}", tied.id())).unwrap();
    std::process::exit(0);
}

#[cfg(windows)]
#[test]
fn what_the_launcher_starts_ends_with_it_unless_it_leaves() {
    let _forks = crate::forks();
    let dir = folder("tie");
    let ids = dir.join("ids");
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact".to_owned(),
            "launcher::tests::tie_child".to_owned(),
            format!("{TIE_CHILD}{}", ids.display()),
        ])
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .status()
        .unwrap();
    assert!(status.success());
    let ids = fs::read_to_string(&ids).unwrap();
    let (tied, left) = ids.split_once(' ').unwrap();
    let (tied, left): (u32, u32) = (tied.parse().unwrap(), left.parse().unwrap());
    assert!(wait_for(Duration::from_secs(5), || !crate::proc::alive(
        tied
    )));
    if left != 0 {
        assert!(crate::proc::alive(left));
        crate::proc::stop(left, crate::proc::Stop::Hard).unwrap();
    }
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(unix)]
#[test]
fn unix_has_no_launcher() {
    assert_eq!(
        tie_children().unwrap_err().kind(),
        io::ErrorKind::Unsupported
    );
}
