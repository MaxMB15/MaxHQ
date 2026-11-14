// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::allow::Platform;
use super::*;
use crate::testing::TempDir;

/// A stand-in for agent-device, run by `/bin/sh` the way `node` runs the
/// real one. It logs its arguments, writes `daemon.json` for `devices`, and
/// answers the commands the tests use.
const FAKE: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$LOG"
case "$1" in
devices)
  if [ -n "$NO_DAEMON" ]; then echo "no simulator found" >&2; exit 3; fi
  printf '{"httpPort":%s,"token":"secret"}' "$PORT" > "$AGENT_DEVICE_STATE_DIR/daemon.json"
  echo '[]';;
screenshot) printf 'PNG' > "$2";;
close) if [ -n "$GONE" ]; then echo "SESSION_NOT_FOUND: no session" >&2; exit 1; fi;;
install) if [ -n "$BAD_APP" ]; then echo "not an app" >&2; exit 1; fi;;
snapshot)
  echo "cwd=$(pwd)"
  echo "base=${AGENT_DEVICE_DAEMON_BASE_URL:-none}"
  echo "idle=$AGENT_DEVICE_DAEMON_IDLE_TIMEOUT_MS";;
wait) sleep 30;;
esac
"#;

/// Writes an executable script.
pub(crate) fn script(path: &Path, text: &str) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    path.to_owned()
}

/// A fake `npm` that installs [`FAKE`] where the real package's entry
/// goes, and counts its runs in `count`.
fn npm(dir: &Path, count: &Path) -> PathBuf {
    let template = script(&dir.join("template.mjs"), FAKE);
    script(
        &dir.join("npm"),
        &format!(
            "#!/bin/sh\necho run >> '{}'\n[ \"$1 $2\" = 'install --prefix' ] || exit 9\n\
             [ \"$6\" = 'agent-device@{VERSION}' ] || exit 9\n\
             mkdir -p \"$3/node_modules/agent-device/bin\"\n\
             cp '{}' \"$3/node_modules/agent-device/bin/agent-device.mjs\"\n",
            count.display(),
            template.display()
        ),
    )
}

/// A loopback server that answers every request with 200, as the daemon's
/// `/health` does.
async fn health() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
            });
        }
    });
    port
}

pub(crate) struct Fixture {
    pub dir: TempDir,
    pub config: Config,
}

impl Fixture {
    pub async fn new(extra: &[(&str, &str)]) -> Self {
        let dir = TempDir::new();
        let port = health().await;
        let mut config = Config::under(dir.path(), "local");
        config.node = PathBuf::from("/bin/sh");
        config.npm = npm(dir.path(), &dir.path().join("npm-runs"));
        config.env = vec![
            (
                "LOG".to_owned(),
                dir.path().join("log").display().to_string(),
            ),
            ("PORT".to_owned(), port.to_string()),
        ];
        config.env.extend(
            extra
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
        );
        Self { dir, config }
    }

    fn device(&self) -> AgentDevice {
        AgentDevice::new(self.config.clone())
    }

    pub fn log(&self) -> Vec<String> {
        fs::read_to_string(self.dir.path().join("log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    pub fn npm_runs(&self) -> usize {
        fs::read_to_string(self.dir.path().join("npm-runs"))
            .unwrap_or_default()
            .lines()
            .count()
    }
}

fn ios() -> Target {
    Target {
        platform: Platform::Ios,
        id: "SIM-1".to_owned(),
    }
}

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

#[test]
fn the_allowlist_passes_lease_commands() {
    for command in [
        &["snapshot", "-i"][..],
        &["press", "@e3"],
        &["fill", "@e2", "hello", "--", "--platform"],
        &["screenshot", "shots/one.png"],
        &["screenshot", "--", "shot.png"],
        &["screenshot", "--fullscreen", "shot.png"],
        &["open", "com.example.app"],
        &["open", "Expo Go", "exp://127.0.0.1:8081"],
        &["open", "com.example.app", "--relaunch", "--wait", "500"],
        &["fill", "@e2", "hello", "--", "--force"],
    ] {
        assert_eq!(allow::check(&args(command)), Ok(()), "{command:?}");
    }
}

#[test]
fn the_allowlist_refuses_flags_that_write_files_or_change_what_an_app_loads() {
    let refusal = |flag: &str| {
        format!(
            "A lease can't pass `{flag}`. It can write outside the lease's folder or change what the app loads. To open a dev server, use open <app> <url>."
        )
    };
    for flag in [
        "--save-script",
        "--launch-console",
        "--launch-args",
        "--force",
        "--overwrite",
        "--bundle-url",
        "--launch-url",
        "--metro-host",
        "--metro-port",
    ] {
        let apart = args(&["open", "com.x", flag, "/Users/someone/.zshrc"]);
        assert_eq!(allow::check(&apart), Err(refusal(flag)), "{apart:?}");
        let joined = format!("{flag}=/Users/someone/.zshrc");
        let joined = args(&["open", "com.x", &joined]);
        assert_eq!(allow::check(&joined), Err(refusal(flag)), "{joined:?}");
    }
    for (command, flag) in [
        (
            &[
                "open",
                "com.x",
                "--save-script=/Users/someone/.zshrc",
                "--overwrite",
            ][..],
            "--save-script",
        ),
        (
            &[
                "open",
                "com.x",
                "--save-script",
                "/Users/someone/.zshrc",
                "--force",
            ],
            "--save-script",
        ),
        (
            &["close", "--save-script", "/Users/someone/.zshrc"],
            "--save-script",
        ),
    ] {
        assert_eq!(
            allow::check(&args(command)),
            Err(refusal(flag)),
            "{command:?}"
        );
    }
}

#[test]
fn a_screenshot_stays_in_the_lease_folder_after_a_double_dash_too() {
    for command in [
        &["screenshot", "--", "/Users/someone/.zshrc"][..],
        &["screenshot", "--", "../../x.png"],
        &["screenshot", "--", "--", "/x"],
        &["screenshot", "--fullscreen", "--", "/x"],
        &["screenshot", "~/.zshrc"],
        &["screenshot", "--", "~/.zshrc"],
    ] {
        let err = allow::check(&args(command)).unwrap_err();
        assert!(err.starts_with("Save the screenshot"), "{command:?}: {err}");
    }
}

#[test]
fn the_allowlist_refuses_other_commands_and_flags() {
    let refused = |command: &[&str]| allow::check(&args(command)).unwrap_err();
    assert!(refused(&[]).contains("snapshot, press"));
    assert!(refused(&["daemon", "stop"]).starts_with("`daemon` isn't a command"));
    assert!(refused(&["connect"]).contains("isn't a command a lease can run"));
    for flag in [
        "--platform",
        "--udid=X",
        "-s",
        "--config",
        "--session",
        "--session-lock",
        "--daemon-base-url",
        "--state-dir",
    ] {
        let err = refused(&["snapshot", flag]);
        let name = flag.split('=').next().unwrap();
        assert_eq!(
            err,
            format!(
                "MaxHQ sets `{name}` itself, or keeps it for the device's host, so leave it out."
            )
        );
    }
    for path in ["/tmp/shot.png", "../shot.png", "a/../../b.png"] {
        assert!(refused(&["screenshot", path]).starts_with("Save the screenshot"));
    }
}

#[test]
fn bound_flags_go_before_a_double_dash() {
    assert_eq!(
        allow::bind(&args(&["fill", "@e1", "--", "-x"]), &["--session", "s"]),
        args(&["fill", "@e1", "--session", "s", "--", "-x"])
    );
    assert_eq!(
        allow::bind(
            &args(&["home"]),
            &["--platform", "android", "--serial", "emu"]
        ),
        args(&["home", "--platform", "android", "--serial", "emu"])
    );
    let android = Target {
        platform: Platform::Android,
        id: "emulator-5554".to_owned(),
    };
    assert_eq!(
        android.args(),
        ["--platform", "android", "--serial", "emulator-5554"]
    );
}

#[tokio::test]
async fn a_command_keeps_its_exit_code_and_output() {
    let mut command = std::process::Command::new("/bin/sh");
    command.args(["-c", "echo out; echo err >&2; exit 3"]);
    let ran = process::run(command, Duration::from_secs(10), 1024)
        .await
        .unwrap();
    assert_eq!(ran.exit_code, Some(3));
    assert_eq!(
        (ran.stdout.as_str(), ran.stderr.as_str()),
        ("out\n", "err\n")
    );
    assert!(!ran.truncated && !ran.timed_out);
    assert_eq!(process::last_line(&ran), Some("err"));
}

#[tokio::test]
async fn output_past_the_cap_is_cut() {
    let mut command = std::process::Command::new("/bin/sh");
    command.args(["-c", "head -c 100000 /dev/zero | tr '\\000' a"]);
    let ran = process::run(command, Duration::from_secs(10), 10)
        .await
        .unwrap();
    assert_eq!(ran.stdout, "aaaaaaaaaa");
    assert!(ran.truncated);
}

#[tokio::test]
async fn a_command_past_its_time_stops_with_its_children() {
    let mut command = std::process::Command::new("/bin/sh");
    command.args(["-c", "sleep 30 & echo started; sleep 30"]);
    let began = std::time::Instant::now();
    let ran = process::run(command, Duration::from_millis(300), 1024)
        .await
        .unwrap();
    assert!(ran.timed_out);
    assert_eq!(ran.exit_code, None);
    assert_eq!(ran.stdout, "started\n");
    assert!(began.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn install_runs_npm_once_then_reuses_the_install() {
    let fixture = Fixture::new(&[]).await;
    let paths = install::ensure(&fixture.config.tools, &fixture.config.npm, VERSION)
        .await
        .unwrap();
    assert_eq!(paths, install::Paths::of(&fixture.config.tools, VERSION));
    assert!(paths.entry.is_file());
    assert_eq!(
        fs::read_to_string(paths.dir.join(".install-complete")).unwrap(),
        format!("{VERSION}\n")
    );
    install::ensure(&fixture.config.tools, &fixture.config.npm, VERSION)
        .await
        .unwrap();
    assert_eq!(fixture.npm_runs(), 1);
}

#[tokio::test]
async fn an_incomplete_install_is_redone() {
    let fixture = Fixture::new(&[]).await;
    let paths = install::Paths::of(&fixture.config.tools, VERSION);
    script(&paths.entry, "#!/bin/sh\n");
    install::ensure(&fixture.config.tools, &fixture.config.npm, VERSION)
        .await
        .unwrap();
    assert_eq!(fixture.npm_runs(), 1);
    assert_eq!(fs::read_to_string(&paths.entry).unwrap(), FAKE);
}

#[tokio::test]
async fn a_failed_install_says_why_and_leaves_nothing() {
    let dir = TempDir::new();
    let tools = dir.path().join("tools");
    let failing = script(
        &dir.path().join("npm"),
        "#!/bin/sh\necho 'npm error 404 Not Found' >&2\nexit 1\n",
    );
    let err = install::ensure(&tools, &failing, VERSION)
        .await
        .unwrap_err();
    assert_eq!(
        err,
        "Installing agent-device failed while running npm install (exit code 1): npm error 404 Not Found"
    );
    let empty = script(&dir.path().join("empty-npm"), "#!/bin/sh\n");
    let err = install::ensure(&tools, &empty, VERSION).await.unwrap_err();
    assert_eq!(
        err,
        "Installing agent-device failed while checking the installed entry point."
    );
    let left: Vec<_> = fs::read_dir(tools.join("agent-device")).unwrap().collect();
    assert!(left.is_empty(), "{left:?}");
}

/// Makes a complete install of `version` finished at `at` seconds.
fn installed(tools: &Path, version: &str, at: u64) -> PathBuf {
    let paths = install::Paths::of(tools, version);
    script(&paths.entry, "#!/bin/sh\n");
    let sentinel = paths.dir.join(".install-complete");
    fs::write(&sentinel, version).unwrap();
    let file = fs::File::options().write(true).open(&sentinel).unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(at))
        .unwrap();
    paths.dir
}

#[test]
fn prune_keeps_the_pin_the_last_other_and_what_runs() {
    let dir = TempDir::new();
    let tools = dir.path();
    installed(tools, VERSION, 400);
    installed(tools, "0.20.0", 300);
    let busy = installed(tools, "0.19.0", 200);
    installed(tools, "0.18.0", 100);
    // Not a version, and not complete, so left alone.
    fs::create_dir_all(tools.join("agent-device").join("notes")).unwrap();
    fs::create_dir_all(tools.join("agent-device").join("0.17.0")).unwrap();
    let running = format!(
        "node {}/node_modules/agent-device/bin/agent-device.mjs snapshot\n",
        busy.display()
    );
    let mut removed = install::prune(tools, VERSION, &running);
    removed.sort();
    assert_eq!(removed, ["0.18.0"]);
    let mut left: Vec<String> = fs::read_dir(tools.join("agent-device"))
        .unwrap()
        .map(|item| item.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, ["0.17.0", "0.19.0", "0.20.0", VERSION, "notes"]);
}

#[test]
fn prune_waits_for_the_pin() {
    let dir = TempDir::new();
    installed(dir.path(), "0.20.0", 300);
    installed(dir.path(), "0.18.0", 100);
    assert!(install::prune(dir.path(), VERSION, "").is_empty());
}

#[tokio::test]
async fn a_command_runs_with_the_target_config_and_session() {
    let fixture = Fixture::new(&[]).await;
    let device = fixture.device();
    let ran = device
        .run(
            &ios(),
            "mhq-1",
            &args(&["snapshot", "-i"]),
            Duration::from_secs(30),
        )
        .await
        .unwrap();
    assert_eq!(ran.exit_code, Some(0));
    let folder = fs::canonicalize(fixture.config.state.join("sessions").join("mhq-1")).unwrap();
    assert!(
        ran.stdout.contains(&format!("cwd={}", folder.display())),
        "{}",
        ran.stdout
    );
    assert!(ran.stdout.contains("base=none"));
    assert!(ran.stdout.contains("idle=0"));
    let config = fixture.config.state.join("config.json");
    assert_eq!(
        fixture.log(),
        [
            "devices --json".to_owned(),
            format!(
                "snapshot -i --platform ios --udid SIM-1 --config {} --session mhq-1",
                config.display()
            ),
        ]
    );
    let text = fs::read_to_string(&config).unwrap();
    assert!(text.contains("\"daemonAuthToken\":\"secret\""), "{text}");
    assert_eq!(
        fs::metadata(&config).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[tokio::test]
async fn a_refused_command_never_runs() {
    let fixture = Fixture::new(&[]).await;
    let err = fixture
        .device()
        .run(
            &ios(),
            "mhq-1",
            &args(&["daemon", "stop"]),
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
    assert!(err.starts_with("`daemon` isn't"));
    assert!(fixture.log().is_empty());
    assert_eq!(fixture.npm_runs(), 0);
}

#[tokio::test]
async fn commands_reuse_a_healthy_daemon_and_restart_a_gone_one() {
    let fixture = Fixture::new(&[]).await;
    let device = fixture.device();
    let snapshot = args(&["snapshot"]);
    device
        .run(&ios(), "a", &snapshot, Duration::from_secs(30))
        .await
        .unwrap();
    device
        .run(&ios(), "a", &snapshot, Duration::from_secs(30))
        .await
        .unwrap();
    // A second maxd, or this one after a restart, finds it up too.
    fixture
        .device()
        .run(&ios(), "a", &snapshot, Duration::from_secs(30))
        .await
        .unwrap();
    let starts = |log: &[String]| {
        log.iter()
            .filter(|line| line.starts_with("devices"))
            .count()
    };
    assert_eq!(starts(&fixture.log()), 1);
    // The daemon died and left its file behind.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = dead.local_addr().unwrap().port();
    drop(dead);
    fs::write(
        fixture.config.state.join("daemon.json"),
        format!(r#"{{"httpPort":{port},"token":"old"}}"#),
    )
    .unwrap();
    device
        .run(&ios(), "a", &snapshot, Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(starts(&fixture.log()), 2);
    assert_eq!(fixture.npm_runs(), 1);
}

#[tokio::test]
async fn a_daemon_that_cant_start_says_why() {
    let fixture = Fixture::new(&[("NO_DAEMON", "1")]).await;
    let err = fixture
        .device()
        .run(&ios(), "a", &args(&["home"]), Duration::from_secs(5))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        "agent-device's daemon didn't start: no simulator found"
    );
}

#[tokio::test]
async fn a_long_command_stops_at_its_time() {
    let fixture = Fixture::new(&[]).await;
    let ran = fixture
        .device()
        .run(
            &ios(),
            "a",
            &args(&["wait", "5000"]),
            Duration::from_millis(300),
        )
        .await
        .unwrap();
    assert!(ran.timed_out);
}

#[tokio::test]
async fn a_screenshot_reads_the_picture_and_removes_it() {
    let fixture = Fixture::new(&[]).await;
    let png = fixture.device().screenshot(&ios(), "a").await.unwrap();
    assert_eq!(png, b"PNG");
    let folder = fixture.config.state.join("sessions").join("a");
    assert_eq!(fs::read_dir(folder).unwrap().count(), 0);
    let line = fixture.log().pop().unwrap();
    assert!(
        line.starts_with("screenshot screen-") && line.contains(".png --platform ios"),
        "{line}"
    );
}

#[tokio::test]
async fn an_app_installs_by_its_name_and_path() {
    let fixture = Fixture::new(&[]).await;
    fixture
        .device()
        .install_app(&ios(), "a", Path::new("/uploads/Demo.app"))
        .await
        .unwrap();
    let line = fixture.log().pop().unwrap();
    assert!(
        line.starts_with("install Demo /uploads/Demo.app --platform ios"),
        "{line}"
    );
    let failing = Fixture::new(&[("BAD_APP", "1")]).await;
    let err = failing
        .device()
        .install_app(&ios(), "a", Path::new("/uploads/Demo.app"))
        .await
        .unwrap_err();
    assert_eq!(err, "agent-device's install failed: not an app");
}

#[tokio::test]
async fn an_apps_file_name_cant_read_as_a_flag() {
    let fixture = Fixture::new(&[]).await;
    fixture
        .device()
        .install_app(&ios(), "a", Path::new("/uploads/--save-script=x.apk"))
        .await
        .unwrap();
    let line = fixture.log().pop().unwrap();
    assert!(
        line.starts_with("install app /uploads/--save-script=x.apk --platform ios"),
        "{line}"
    );
    let err = fixture
        .device()
        .install_app(&ios(), "a", Path::new("--save-script=x.apk"))
        .await
        .unwrap_err();
    assert_eq!(err, "--save-script=x.apk isn't a full path to an app.");
    let log = fixture.log();
    let installs = log.iter().filter(|line| line.starts_with("install"));
    assert_eq!(installs.count(), 1, "{log:?}");
}

#[test]
fn plain_app_names_stay_and_others_get_a_fixed_one() {
    for (stem, name) in [
        ("Demo", "Demo"),
        ("My App", "My App"),
        ("app-debug", "app-debug"),
        ("com.example_1", "com.example_1"),
        ("--force", "app"),
        ("-x", "app"),
        (".hidden", "app"),
        ("a=b", "app"),
        ("a\nb", "app"),
        ("", "app"),
    ] {
        assert_eq!(allow::install_name(stem), name, "{stem:?}");
    }
}

#[tokio::test]
async fn close_ends_the_session_and_removes_its_files() {
    let fixture = Fixture::new(&[]).await;
    let device = fixture.device();
    device
        .run(&ios(), "a", &args(&["home"]), Duration::from_secs(30))
        .await
        .unwrap();
    let folder = fixture.config.state.join("sessions").join("a");
    fs::write(folder.join("shot.png"), "x").unwrap();
    device.close(&ios(), "a").await.unwrap();
    assert!(!folder.exists());
    assert!(fixture
        .log()
        .pop()
        .unwrap()
        .starts_with("close --platform ios"));
}

#[tokio::test]
async fn closing_a_session_that_ended_counts_as_closed() {
    let fixture = Fixture::new(&[("GONE", "1")]).await;
    fixture.device().close(&ios(), "a").await.unwrap();
}

#[tokio::test]
async fn stop_stops_the_daemon_in_its_state_folder() {
    let fixture = Fixture::new(&[]).await;
    let device = fixture.device();
    device.stop().await;
    assert!(fixture.log().is_empty());
    device.start().await.unwrap();
    device.stop().await;
    assert_eq!(
        fixture.log().pop().unwrap(),
        format!("daemon stop --state-dir {}", fixture.config.state.display())
    );
}

#[test]
fn commands_drop_variables_that_pick_another_daemon() {
    let device = AgentDevice::new(Config::under(Path::new("/data"), "local"));
    let command = device.command(Path::new("/entry.mjs"), &args(&["home"]));
    let envs: Vec<_> = command.get_envs().collect();
    for name in daemon::STRIPPED {
        assert!(envs.contains(&(std::ffi::OsStr::new(name), None)), "{name}");
    }
    assert!(envs.contains(&(
        std::ffi::OsStr::new("AGENT_DEVICE_STATE_DIR"),
        Some(std::ffi::OsStr::new("/data/agent-device/local"))
    )));
}

#[test]
fn a_wrap_runs_node_under_it() {
    let mut config = Config::under(Path::new("/data"), "local");
    config.wrap = args(&["/usr/bin/sandbox-exec", "-p", "(version 1)"]);
    let device = AgentDevice::new(config);
    let command = device.command(Path::new("/entry.mjs"), &args(&["home"]));
    assert_eq!(command.get_program(), "/usr/bin/sandbox-exec");
    let words: Vec<_> = command.get_args().collect();
    assert_eq!(words, ["-p", "(version 1)", "node", "/entry.mjs", "home"]);
}

#[test]
fn a_lease_has_a_session_of_its_own() {
    let lease = maxhq_protocol::ids::LeaseId(uuid::Uuid::nil());
    assert_eq!(session(lease), "mhq-00000000000000000000000000000000");
}
