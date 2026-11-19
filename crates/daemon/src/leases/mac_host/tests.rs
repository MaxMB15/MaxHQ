// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::collections::HashMap;
use std::fs;
use std::sync::{Mutex, PoisonError};

use maxhq_protocol::ids::LeaseId;

use super::*;
use crate::agent_device::tests::{script, Fixture};
use crate::leases::detect::IosRuntime;
use crate::testing::TempDir;

const RUNTIME: &str = "com.apple.CoreSimulator.SimRuntime.iOS-26-4";
const IPHONE: &str = "com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro";

/// What `simctl list runtimes --json` prints, cut down.
const RUNTIMES: &str = r#"{"runtimes": [
  {"identifier": "com.apple.CoreSimulator.SimRuntime.tvOS-26-4",
   "supportedDeviceTypes": [{"identifier": "tv", "productFamily": "Apple TV"}]},
  {"identifier": "com.apple.CoreSimulator.SimRuntime.iOS-26-4",
   "supportedDeviceTypes": [
     {"identifier": "ipad", "productFamily": "iPad"},
     {"identifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro", "productFamily": "iPhone"},
     {"identifier": "iphone-older", "productFamily": "iPhone"}]}
]}"#;

/// A fake `xcrun simctl` with a list of devices that `create` and
/// `delete` change. It logs each call.
#[derive(Default)]
struct FakeXcrun {
    log: Mutex<Vec<String>>,
    devices: Mutex<Vec<String>>,
    fails: Mutex<HashMap<&'static str, String>>,
    created: Mutex<u32>,
}

fn held<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl FakeXcrun {
    fn log(&self) -> Vec<String> {
        held(&self.log).clone()
    }

    fn fail(&self, verb: &'static str, error: &str) {
        held(&self.fails).insert(verb, error.to_owned());
    }

    fn answer(&self, args: &[&str]) -> Result<String, String> {
        if let Some(error) = held(&self.fails).get(args[0]) {
            return Err(error.clone());
        }
        Ok(match args {
            ["list", "runtimes", ..] => RUNTIMES.to_owned(),
            ["list", "devices", ..] => {
                let devices: Vec<_> = held(&self.devices)
                    .iter()
                    .map(|udid| serde_json::json!({ "udid": udid, "state": "Shutdown" }))
                    .collect();
                serde_json::json!({ "devices": { RUNTIME: devices } }).to_string()
            }
            ["create", ..] => {
                let mut created = held(&self.created);
                *created += 1;
                let udid = format!("NEW-{created}");
                held(&self.devices).push(udid.clone());
                format!("{udid}\n")
            }
            ["delete", udid] => {
                held(&self.devices).retain(|known| known != udid);
                String::new()
            }
            _ => String::new(),
        })
    }
}

impl Xcrun for FakeXcrun {
    fn simctl<'a>(
        &'a self,
        args: &'a [&'a str],
        _timeout: Duration,
    ) -> BoxFuture<'a, Result<String, String>> {
        held(&self.log).push(args.join(" "));
        let answer = self.answer(args);
        async move { answer }.boxed()
    }
}

/// A Mac host on a fake `xcrun` and a fake agent-device, which detection
/// finds ready unless `setup` says what's missing.
struct Mac {
    fixture: Fixture,
    xcrun: Arc<FakeXcrun>,
    host: MacHost,
}

impl Mac {
    async fn new(setup: Option<&str>) -> Self {
        let fixture = Fixture::new(&[]).await;
        // Beside the fake `npm`, as detection's `node` is beside the real one.
        let node = script(
            &fixture.dir.path().join("node"),
            "#!/bin/sh\nexec /bin/sh \"$@\"\n",
        );
        let found = match setup {
            Some(setup) => Err(setup.to_owned()),
            None => Ok(Ready::MacHost {
                runtime: IosRuntime {
                    identifier: RUNTIME.to_owned(),
                    name: "iOS 26.4".to_owned(),
                },
                node,
            }),
        };
        let find: Find = Arc::new(move || {
            let found = found.clone();
            async move { found }.boxed()
        });
        let xcrun = Arc::new(FakeXcrun::default());
        let host = MacHost::new(
            xcrun.clone(),
            find,
            fixture.dir.path().join("host"),
            fixture.config.clone(),
            false,
        );
        Self {
            fixture,
            xcrun,
            host,
        }
    }

    fn kept(&self) -> Option<String> {
        self.host.udid()
    }

    /// A built host and a lease started on it.
    async fn leased(&self) -> LeaseTarget {
        self.host.build().await.unwrap();
        let lease = LeaseId(uuid::Uuid::nil());
        let target = self.host.start(lease).await.unwrap();
        LeaseTarget { lease, target }
    }
}

#[test]
fn the_profile_closes_the_home_folder_but_what_agent_device_needs() {
    let lines: Vec<&str> = SEATBELT_PROFILE.lines().map(str::trim).collect();
    let at = |line: &str| {
        lines
            .iter()
            .position(|found| *found == line)
            .unwrap_or_else(|| panic!("no {line} in the profile"))
    };
    let deny = at(r#"(deny file-read* file-write* (subpath (param "HOME")))"#);
    assert_eq!(at("(allow default)"), 1);
    // Seatbelt takes the last rule that matches, so the openings come after.
    assert!(at(r#"(allow file-read-metadata (subpath (param "HOME")))"#) > deny);
    let writes = at("(allow file-read* file-write*");
    let reads = at("(allow file-read*");
    assert!(writes > deny && reads > writes);
    let opened: Vec<&str> = lines[writes + 1..]
        .iter()
        .copied()
        .filter(|line| line.starts_with("(subpath"))
        .collect();
    assert_eq!(
        opened,
        [
            r#"(subpath (param "STATE"))"#,
            r#"(subpath (string-append (param "HOME") "/.agent-device"))"#,
            r#"(subpath (string-append (param "HOME") "/Library/Developer"))"#,
            r#"(subpath (string-append (param "HOME") "/Library/Caches/com.apple.dt.xcodebuild"))"#,
            r#"(subpath (string-append (param "HOME") "/Library/Logs/CoreSimulator")))"#,
            r#"(subpath (param "TOOLS"))"#,
            r#"(subpath (param "NODE")))"#,
        ]
    );
    for private in [
        ".ssh",
        "Documents",
        "Desktop",
        "Keychains",
        "Mail",
        ".claude",
        ".codex",
        "Code",
    ] {
        assert!(!SEATBELT_PROFILE.contains(private), "{private}");
    }
    assert!(!SEATBELT_PROFILE.contains("(deny default)"));
}

#[test]
fn sandbox_exec_gets_the_profile_and_each_real_path() {
    let fixture = TempDir::new();
    let real = fs::canonicalize(fixture.path()).unwrap();
    let words = seatbelt(&[("HOME", fixture.path()), ("STATE", Path::new("/no/such"))]);
    assert_eq!(
        words,
        [
            "/usr/bin/sandbox-exec".to_owned(),
            "-D".to_owned(),
            format!("HOME={}", real.display()),
            "-D".to_owned(),
            "STATE=/no/such".to_owned(),
            "-p".to_owned(),
            SEATBELT_PROFILE.to_owned(),
        ]
    );
}

#[tokio::test]
async fn create_makes_the_newest_iphone_on_the_runtime() {
    let xcrun = FakeXcrun::default();
    assert_eq!(iphone_for(&xcrun, RUNTIME).await.unwrap(), IPHONE);
    let udid = create(&xcrun, SIMULATOR_NAME, RUNTIME).await.unwrap();
    assert_eq!(udid, "NEW-1");
    assert_eq!(
        xcrun.log().last().unwrap(),
        &format!("create {SIMULATOR_NAME} {IPHONE} {RUNTIME}")
    );
    let tv = "com.apple.CoreSimulator.SimRuntime.tvOS-26-4";
    assert!(iphone_for(&xcrun, tv)
        .await
        .unwrap_err()
        .contains("no iPhone"));
    assert!(iphone_for(&xcrun, "gone").await.is_err());
}

#[tokio::test]
async fn shutting_down_one_thats_shut_down_counts() {
    let xcrun = FakeXcrun::default();
    xcrun.fail(
        "shutdown",
        "simctl shutdown failed: Unable to shutdown device in current state: Shutdown",
    );
    shutdown(&xcrun, "SIM-1").await.unwrap();
    xcrun.fail("shutdown", "simctl shutdown failed: Invalid device: SIM-1");
    assert!(shutdown(&xcrun, "SIM-1").await.is_err());
}

#[tokio::test]
async fn detection_answers_for_the_host() {
    let mac = Mac::new(Some("Xcode command line tools were not found.")).await;
    assert_eq!(
        mac.host.detect().await.as_deref(),
        Some("Xcode command line tools were not found.")
    );
    assert!(mac.host.build().await.is_err());
    assert_eq!(Mac::new(None).await.host.detect().await, None);
}

#[tokio::test]
async fn a_build_installs_agent_device_and_creates_its_own_simulator() {
    let mac = Mac::new(None).await;
    held(&mac.xcrun.devices).push("YOURS".to_owned());
    let built = mac.host.build().await.unwrap();
    assert_eq!(built.agent_device.as_deref(), Some(VERSION));
    assert_eq!(mac.kept().as_deref(), Some("NEW-1"));
    assert_eq!(
        mac.xcrun.log(),
        [
            "list runtimes --json".to_owned(),
            format!("create {SIMULATOR_NAME} {IPHONE} {RUNTIME}"),
        ]
    );
    assert_eq!(mac.fixture.npm_runs(), 1);
}

#[tokio::test]
async fn a_rebuild_deletes_only_the_simulator_it_made() {
    let mac = Mac::new(None).await;
    held(&mac.xcrun.devices).push("YOURS".to_owned());
    mac.host.build().await.unwrap();
    mac.host.build().await.unwrap();
    assert_eq!(mac.kept().as_deref(), Some("NEW-2"));
    assert_eq!(*held(&mac.xcrun.devices), ["YOURS", "NEW-2"]);
    let log = mac.xcrun.log();
    assert!(log.contains(&"delete NEW-1".to_owned()), "{log:?}");
    assert!(!log.iter().any(|call| call.contains("YOURS")), "{log:?}");
}

#[tokio::test]
async fn a_rebuild_skips_a_simulator_thats_gone() {
    let mac = Mac::new(None).await;
    mac.host.build().await.unwrap();
    held(&mac.xcrun.devices).clear();
    mac.host.build().await.unwrap();
    assert!(!mac
        .xcrun
        .log()
        .iter()
        .any(|call| call.starts_with("delete")));
}

#[tokio::test]
async fn a_lease_boots_the_simulator_and_drives_it() {
    let mac = Mac::new(None).await;
    let lease = LeaseId(uuid::Uuid::nil());
    assert!(mac
        .host
        .start(lease)
        .await
        .unwrap_err()
        .contains("isn't built"));
    let on = mac.leased().await;
    assert_eq!(on.target, "NEW-1");
    assert_eq!(mac.xcrun.log().last().unwrap(), "bootstatus NEW-1 -b");
    let command = ["snapshot".to_owned(), "-i".to_owned()];
    let ran = mac
        .host
        .run(&on, &command, Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(ran.exit_code, Some(0));
    let session = agent_device::session(on.lease);
    let ran = mac.fixture.log().pop().unwrap();
    assert!(
        ran.starts_with("snapshot -i --platform ios --udid NEW-1 --config"),
        "{ran}"
    );
    assert!(ran.ends_with(&format!("--session {session}")), "{ran}");
    assert_eq!(mac.host.screenshot(&on).await.unwrap(), b"PNG");
}

#[tokio::test]
async fn an_install_copies_the_app_where_agent_device_may_read_it() {
    let mac = Mac::new(None).await;
    let on = mac.leased().await;
    let app = mac.fixture.dir.path().join("build").join("Sample.app");
    fs::create_dir_all(&app).unwrap();
    fs::write(app.join("Info.plist"), "plist").unwrap();
    let said = mac.host.install(&on, app.clone()).await.unwrap();
    assert_eq!(said, "Installed Sample.app on the Simulator.");
    let apps = mac.fixture.config.state.join(APPS_DIR);
    let copy = apps
        .join(agent_device::session(on.lease))
        .join("Sample.app");
    let ran = mac.fixture.log().pop().unwrap();
    assert!(
        ran.starts_with(&format!("install Sample {} --platform ios", copy.display())),
        "{ran}"
    );
    assert!(!copy.exists() && app.join("Info.plist").exists());
}

#[tokio::test]
async fn closing_ends_the_session_then_erases_the_simulator() {
    let mac = Mac::new(None).await;
    let on = mac.leased().await;
    mac.host.close(&on).await.unwrap();
    assert!(mac
        .fixture
        .log()
        .pop()
        .unwrap()
        .starts_with("close --platform ios --udid NEW-1"));
    let log = mac.xcrun.log();
    assert_eq!(log[log.len() - 2..], ["shutdown NEW-1", "erase NEW-1"]);
}

#[tokio::test]
async fn closing_says_when_the_simulator_wont_erase() {
    let mac = Mac::new(None).await;
    let on = mac.leased().await;
    mac.xcrun.fail("erase", "simctl erase failed: busy");
    assert_eq!(
        mac.host.close(&on).await.unwrap_err(),
        "simctl erase failed: busy"
    );
    assert!(mac.xcrun.log().contains(&"erase NEW-1".to_owned()));
}

#[tokio::test]
async fn a_reset_erases_the_simulator_or_says_its_gone() {
    let mac = Mac::new(None).await;
    assert!(mac.host.reset().await.unwrap_err().contains("isn't built"));
    mac.host.build().await.unwrap();
    mac.host.reset().await.unwrap();
    let log = mac.xcrun.log();
    assert_eq!(log[log.len() - 2..], ["shutdown NEW-1", "erase NEW-1"]);
    assert!(mac
        .fixture
        .log()
        .iter()
        .any(|ran| ran.starts_with("daemon stop")));
    held(&mac.xcrun.devices).clear();
    assert!(mac.host.reset().await.unwrap_err().contains("gone"));
}
