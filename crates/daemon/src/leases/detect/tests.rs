// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the setup
// sentences these check for the iOS and Android rules. The fake device
// trees are MaxHQ's.

//! Detection against fake devices: fake trees, programs and answers, so
//! nothing here runs `xcrun`, `node` or Incus.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use maxhq_protocol::leases::{host_id, TestHostState};
use maxhq_protocol::state::{Change, Entity, EntityKey};

use super::*;
use crate::db::Db;
use crate::leases::fake::FakeBackend;
use crate::leases::TestHosts;
use crate::secrets::testing::hub;
use crate::session::clock::FakeClock;

const HOME: &str = "/home/someone";

/// A device made of fake files, programs and answers.
struct Fake {
    os: &'static str,
    arch: &'static str,
    env: HashMap<String, String>,
    files: BTreeSet<PathBuf>,
    closed: BTreeSet<PathBuf>,
    links: HashMap<PathBuf, PathBuf>,
    answers: HashMap<String, String>,
    incus: Result<(), String>,
    ran: Mutex<Vec<String>>,
}

impl Fake {
    fn new(os: &'static str, arch: &'static str) -> Self {
        Self {
            os,
            arch,
            env: HashMap::new(),
            files: BTreeSet::new(),
            closed: BTreeSet::new(),
            links: HashMap::new(),
            answers: HashMap::new(),
            incus: Ok(()),
            ran: Mutex::new(Vec::new()),
        }
    }

    /// An Apple silicon Mac on macOS 15 with no Xcode and no Node.
    fn mac() -> Self {
        Self::new("macos", "aarch64")
            .program("sw_vers")
            .answer("sw_vers -productVersion", "15.1")
    }

    /// An x86_64 Linux device with nothing on it.
    fn linux() -> Self {
        Self::new("linux", "x86_64")
    }

    fn file(mut self, path: &str) -> Self {
        self.files.insert(path.into());
        self
    }

    fn files(self, paths: &[&str]) -> Self {
        paths.iter().fold(self, |fake, path| fake.file(path))
    }

    fn env(mut self, name: &str, value: &str) -> Self {
        self.env.insert(name.into(), value.into());
        self
    }

    fn link(mut self, from: &str, to: &str) -> Self {
        self.links.insert(from.into(), to.into());
        self.file(from)
    }

    /// A program at `/usr/bin/<name>`.
    fn program(self, name: &str) -> Self {
        self.file(&format!("/usr/bin/{name}"))
    }

    /// What `/usr/bin/<command>` prints.
    fn answer(mut self, command: &str, output: &str) -> Self {
        self.answers
            .insert(format!("/usr/bin/{command}"), output.into());
        self
    }

    fn node(self, version: &str) -> Self {
        self.program("node")
            .answer("node --version", &format!("v{version}"))
    }

    fn xcode(self, runtimes: &str) -> Self {
        self.program("xcrun")
            .answer("xcrun simctl list runtimes --json", runtimes)
    }

    fn ran(&self) -> Vec<String> {
        self.ran.lock().unwrap().clone()
    }
}

impl System for Fake {
    fn os(&self) -> &str {
        self.os
    }

    fn arch(&self) -> &str {
        self.arch
    }

    fn env(&self, name: &str) -> Option<String> {
        self.env.get(name).cloned()
    }

    fn home(&self) -> Option<PathBuf> {
        Some(HOME.into())
    }

    fn exists(&self, path: &Path) -> bool {
        self.files.iter().any(|file| file.starts_with(path))
    }

    fn real_path(&self, path: &Path) -> PathBuf {
        self.links.get(path).cloned().unwrap_or_else(|| path.into())
    }

    fn list(&self, dir: &Path) -> Vec<String> {
        let names: BTreeSet<String> = self
            .files
            .iter()
            .filter_map(|file| file.strip_prefix(dir).ok()?.iter().next())
            .filter_map(|name| name.to_str().map(str::to_owned))
            .collect();
        names.into_iter().collect()
    }

    fn opens(&self, path: &Path) -> bool {
        self.exists(path) && !self.closed.contains(path)
    }

    fn program(&self, name: &str) -> Option<PathBuf> {
        let path = PathBuf::from(format!("/usr/bin/{name}"));
        self.files.contains(&path).then_some(path)
    }

    fn output<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [&'a str],
    ) -> BoxFuture<'a, Option<String>> {
        let command = std::iter::once(program.display().to_string())
            .chain(args.iter().map(|arg| (*arg).to_owned()))
            .collect::<Vec<_>>()
            .join(" ");
        self.ran.lock().unwrap().push(command.clone());
        let answer = self.answers.get(&command).cloned();
        async move { answer }.boxed()
    }

    fn incus<'a>(&'a self, _socket: &'a Path) -> BoxFuture<'a, Result<(), String>> {
        let answer = self.incus.clone();
        async move { answer }.boxed()
    }
}

fn paths() -> Paths {
    Paths {
        vm_base: "/data/test-hosts/vm/base".into(),
        vm_restore: "/data/test-hosts/vm/restore.ipsw".into(),
        vm_build: "/data/test-hosts/vm/build.json".into(),
        incus_socket: "/var/lib/incus/unix.socket".into(),
    }
}

/// What `fake` has for `backend` of `kind`.
async fn found(fake: Fake, kind: TestKind, backend: TestHostBackend) -> Result<Ready, String> {
    Detector::new(fake, paths()).ready(kind, backend).await
}

async fn setup(fake: Fake, kind: TestKind, backend: TestHostBackend) -> String {
    found(fake, kind, backend)
        .await
        .expect_err("something is missing")
}

async fn ios(fake: Fake) -> Result<Ready, String> {
    found(fake, TestKind::IosSimulator, TestHostBackend::MacHost).await
}

async fn android(fake: Fake) -> Result<Ready, String> {
    found(
        fake,
        TestKind::AndroidEmulator,
        TestHostBackend::HostEmulator,
    )
    .await
}

const RUNTIMES: &str = r#"{"runtimes": [
    {"identifier": "com.apple.CoreSimulator.SimRuntime.iOS-17-5", "name": "iOS 17.5",
     "version": "17.5", "platform": "iOS", "isAvailable": true},
    {"identifier": "com.apple.CoreSimulator.SimRuntime.iOS-18-2", "name": "iOS 18.2",
     "version": "18.2", "platform": "iOS", "isAvailable": true},
    {"identifier": "com.apple.CoreSimulator.SimRuntime.iOS-18-4", "name": "iOS 18.4",
     "version": "18.4", "platform": "iOS", "isAvailable": false},
    {"identifier": "com.apple.CoreSimulator.SimRuntime.watchOS-11-2", "name": "watchOS 11.2",
     "version": "11.2", "platform": "watchOS", "isAvailable": true}
]}"#;

const NO_RUNTIMES: &str = r#"{"runtimes": [
    {"identifier": "com.apple.CoreSimulator.SimRuntime.tvOS-18-2", "name": "tvOS 18.2",
     "version": "18.2", "platform": "tvOS", "isAvailable": true}
]}"#;

#[tokio::test]
async fn ios_needs_macos_then_xcode_then_its_simulator_tools() {
    let not_mac = ios(Fake::linux()).await;
    assert_eq!(not_mac, Err("iOS Simulators need macOS with Xcode.".into()));
    let no_xcrun = ios(Fake::mac()).await;
    assert_eq!(
        no_xcrun,
        Err("Xcode command line tools were not found.".into())
    );
    let silent = ios(Fake::mac().program("xcrun")).await.unwrap_err();
    assert!(
        silent.starts_with("Xcode's Simulator tools didn't answer."),
        "{silent}"
    );
}

#[tokio::test]
async fn ios_needs_an_available_ios_runtime() {
    let none = ios(Fake::mac().xcode(NO_RUNTIMES)).await.unwrap_err();
    assert!(
        none.starts_with("Xcode has no iOS Simulator runtime."),
        "{none}"
    );
    assert!(none.ends_with(NOT_YET_FOR_YOU), "{none}");
    let garbled = ios(Fake::mac().xcode("not json")).await.unwrap_err();
    assert_eq!(garbled, none);
}

#[tokio::test]
async fn ios_takes_the_newest_available_runtime_and_node() {
    let ready = ios(Fake::mac().xcode(RUNTIMES).node("22.11.0")).await;
    assert_eq!(
        ready,
        Ok(Ready::MacHost {
            runtime: IosRuntime {
                identifier: "com.apple.CoreSimulator.SimRuntime.iOS-18-2".into(),
                name: "iOS 18.2".into(),
            },
            node: "/usr/bin/node".into(),
        })
    );
}

#[tokio::test]
async fn a_backend_another_kind_has_isnt_looked_for() {
    let found = Detector::new(Fake::mac(), paths())
        .ready(TestKind::Linux, TestHostBackend::MacHost)
        .await;
    assert_eq!(found, Err("MaxHQ doesn't look for that test host.".into()));
}

#[test]
fn a_runtime_without_a_platform_is_known_by_its_name() {
    let listed = r#"{"runtimes": [
        {"identifier": "old-ios", "name": "iOS 16.4", "version": "16.4", "isAvailable": true},
        {"identifier": "old-tv", "name": "tvOS 17.0", "version": "17.0", "isAvailable": true}
    ]}"#;
    let runtime = newest_ios_runtime(listed).unwrap();
    assert_eq!(runtime.identifier, "old-ios");
}

#[tokio::test]
async fn node_must_be_there_and_new_enough() {
    let missing = ios(Fake::mac().xcode(RUNTIMES)).await.unwrap_err();
    assert_eq!(
        missing,
        format!(
            "Test hosts need Node.js 22 or later for agent-device. Install it and make sure \
             node is on PATH, then check again. {NOT_YET_FOR_YOU}"
        )
    );
    let old = ios(Fake::mac().xcode(RUNTIMES).node("20.11.1"))
        .await
        .unwrap_err();
    assert!(
        old.starts_with(
            "Test hosts need Node.js 22 or later for agent-device, and this device has 20.11.1."
        ),
        "{old}"
    );
    let mute = ios(Fake::mac().xcode(RUNTIMES).program("node"))
        .await
        .unwrap_err();
    assert!(mute.contains("one that doesn't say its version"), "{mute}");
}

/// A whole SDK at `root`, with one image.
fn with_sdk(fake: Fake, root: &str) -> Fake {
    fake.files(&[
        &format!("{root}/platform-tools/adb"),
        &format!("{root}/emulator/emulator"),
        &format!("{root}/cmdline-tools/latest/bin/avdmanager"),
        &format!("{root}/system-images/android-35/google_apis/x86_64/source.properties"),
        &format!("{root}/system-images/android-35/google_apis/arm64-v8a/source.properties"),
    ])
}

#[tokio::test]
async fn android_without_an_sdk_says_where_to_get_one() {
    let missing = android(Fake::linux()).await.unwrap_err();
    assert_eq!(
        missing,
        format!(
            "Android SDK was not found. Install it with Android Studio or set ANDROID_HOME to \
             your SDK directory. {NOT_YET_FOR_YOU}"
        )
    );
}

#[tokio::test]
async fn android_names_what_the_sdk_lacks_in_order() {
    let root = "/opt/android";
    let named = || Fake::linux().env("ANDROID_HOME", &format!(" {root} "));
    let no_adb = android(named()).await.unwrap_err();
    assert_eq!(
        no_adb,
        format!(
            "Android SDK Platform-Tools are missing from {root}. Install them in Android \
             Studio's SDK Manager. {NOT_YET_FOR_YOU}"
        )
    );
    let adb = format!("{root}/platform-tools/adb");
    let no_emulator = android(named().file(&adb)).await.unwrap_err();
    assert_eq!(
        no_emulator,
        format!(
            "Android Emulator is missing from {root}. Install it in Android Studio's SDK \
             Manager. {NOT_YET_FOR_YOU}"
        )
    );
    let tools = named().files(&[&adb, &format!("{root}/emulator/emulator")]);
    let no_tools = android(tools).await.unwrap_err();
    assert_eq!(
        no_tools,
        format!(
            "Android SDK Command-line Tools (latest) are missing from {root}. Install them in \
             Android Studio's SDK Manager. {NOT_YET_FOR_YOU}"
        )
    );
    let legacy = named().files(&[
        &adb,
        &format!("{root}/emulator/emulator"),
        &format!("{root}/tools/bin/avdmanager"),
    ]);
    let legacy = android(legacy).await.unwrap_err();
    assert!(
        legacy.starts_with(&format!(
            "The Android SDK command-line tools in {root} appear to be an older, unsupported \
             version."
        )),
        "{legacy}"
    );
}

#[tokio::test]
async fn android_sdk_root_is_the_fallback_name() {
    let fake = with_sdk(Fake::linux(), "/opt/sdk")
        .env("ANDROID_SDK_ROOT", "/opt/sdk")
        .file("/dev/kvm")
        .node("22.0.0");
    let Ok(Ready::HostEmulator { sdk, .. }) = android(fake).await else {
        panic!("expected an SDK");
    };
    assert_eq!(sdk.root, PathBuf::from("/opt/sdk"));
}

#[tokio::test]
async fn android_finds_the_usual_folders_before_path() {
    let mac_sdk = format!("{HOME}/Library/Android/sdk");
    let linux_sdk = format!("{HOME}/Android/Sdk");
    let fake = with_sdk(with_sdk(Fake::mac(), &mac_sdk), &linux_sdk)
        .node("24.1.0")
        .env("PATH", "/opt/elsewhere/platform-tools");
    let Ok(Ready::HostEmulator { sdk, .. }) = android(fake).await else {
        panic!("expected an SDK");
    };
    assert_eq!(sdk.root, PathBuf::from(mac_sdk));

    // A usual folder with neither adb nor emulator in it doesn't count.
    let fake = with_sdk(Fake::linux(), &linux_sdk)
        .file(&format!(
            "{HOME}/Library/Android/sdk/licenses/android-sdk-license"
        ))
        .file("/dev/kvm")
        .node("22.0.0");
    let Ok(Ready::HostEmulator { sdk, .. }) = android(fake).await else {
        panic!("expected an SDK");
    };
    assert_eq!(sdk.root, PathBuf::from(linux_sdk));
}

#[tokio::test]
async fn android_finds_the_sdk_maxhq_installed() {
    let ours = format!("{HOME}/.local/lib/maxhq/tools/android");
    let fake = with_sdk(Fake::mac(), &ours).node("24.1.0");
    let Ok(Ready::HostEmulator { sdk, .. }) = android(fake).await else {
        panic!("expected an SDK");
    };
    assert_eq!(sdk.root, PathBuf::from(&ours));

    // Android Studio's comes first.
    let studio = format!("{HOME}/Library/Android/sdk");
    let fake = with_sdk(with_sdk(Fake::mac(), &ours), &studio).node("24.1.0");
    let Ok(Ready::HostEmulator { sdk, .. }) = android(fake).await else {
        panic!("expected an SDK");
    };
    assert_eq!(sdk.root, PathBuf::from(studio));
}

#[tokio::test]
async fn android_follows_an_adb_on_path_to_its_sdk() {
    let fake = with_sdk(Fake::linux(), "/opt/real-sdk")
        .env("PATH", ":/usr/local/bin:/opt/shims")
        .link("/opt/shims/adb", "/opt/real-sdk/platform-tools/adb")
        .file("/dev/kvm")
        .node("22.0.0");
    let ready = android(fake).await;
    assert_eq!(
        ready,
        Ok(Ready::HostEmulator {
            sdk: AndroidSdk {
                root: "/opt/real-sdk".into()
            },
            image: SystemImage {
                package: "system-images;android-35;google_apis;x86_64".into(),
                api: 35,
            },
            node: "/usr/bin/node".into(),
        })
    );
}

#[tokio::test]
async fn android_ignores_an_adb_in_a_relative_folder_on_path() {
    let fake = with_sdk(Fake::linux(), "/opt/real-sdk")
        .env("PATH", "shims")
        .link("shims/adb", "/opt/real-sdk/platform-tools/adb")
        .file("/dev/kvm")
        .node("22.0.0");
    assert!(android(fake).await.is_err());
}

#[tokio::test]
async fn android_takes_the_newest_image_for_the_cpu() {
    let root = "/opt/sdk";
    let image = |name: &str| format!("{root}/system-images/{name}/source.properties");
    let fake = with_sdk(Fake::mac(), root)
        .env("ANDROID_HOME", root)
        .node("22.0.0")
        .files(&[
            &image("android-36/google_apis_playstore/arm64-v8a"),
            &image("android-36/default/arm64-v8a"),
            &image("android-36/aosp_atd/arm64-v8a"),
            &image("android-37/google_apis/x86_64"),
            &image("android-Baklava/google_apis/arm64-v8a"),
        ])
        .file(&format!(
            "{root}/system-images/android-38/google_apis/arm64-v8a/partial"
        ));
    let Ok(Ready::HostEmulator { image, .. }) = android(fake).await else {
        panic!("expected an image");
    };
    assert_eq!(
        image,
        SystemImage {
            package: "system-images;android-36;default;arm64-v8a".into(),
            api: 36,
        }
    );
}

#[tokio::test]
async fn android_without_an_image_for_the_cpu_says_so() {
    let root = "/opt/sdk";
    let fake = Fake::new("linux", "riscv64").env("ANDROID_HOME", root);
    let cpu = android(with_sdk(fake, root)).await.unwrap_err();
    assert_eq!(
        cpu,
        "The Android emulator needs an arm64 or x86_64 processor."
    );

    let fake = Fake::mac().env("ANDROID_HOME", root).files(&[
        &format!("{root}/platform-tools/adb"),
        &format!("{root}/emulator/emulator"),
        &format!("{root}/cmdline-tools/latest/bin/avdmanager"),
        &format!("{root}/system-images/android-35/google_apis/x86_64/source.properties"),
    ]);
    let none = android(fake).await.unwrap_err();
    assert!(
        none.starts_with(&format!(
            "No Android system image for arm64-v8a is in {root}."
        )),
        "{none}"
    );
}

#[tokio::test]
async fn android_on_linux_needs_a_kvm_it_can_open() {
    let root = "/opt/sdk";
    let fake = || with_sdk(Fake::linux().env("ANDROID_HOME", root), root).node("22.0.0");
    let missing = android(fake()).await.unwrap_err();
    assert!(
        missing.starts_with("The Android emulator needs /dev/kvm"),
        "{missing}"
    );
    let mut closed = fake().file("/dev/kvm");
    closed.closed.insert("/dev/kvm".into());
    let closed = android(closed).await.unwrap_err();
    assert!(closed.starts_with("maxd can't open /dev/kvm."), "{closed}");
    assert!(android(fake().file("/dev/kvm")).await.is_ok());

    // Windows has none yet, whatever it has installed.
    let windows = Fake::new("windows", "x86_64").env("ANDROID_HOME", root);
    let windows = with_sdk(windows, root).node("22.0.0");
    let refused = android(windows).await.unwrap_err();
    assert!(refused.contains("aren't on Windows yet"), "{refused}");

    // A Mac has no /dev/kvm, and the emulator needs none there.
    let mac = with_sdk(Fake::mac().env("ANDROID_HOME", root), root).node("22.0.0");
    assert!(android(mac).await.is_ok());
}

#[tokio::test]
async fn the_mac_vm_needs_apple_silicon_macos_14_xcode_and_a_base_or_restore_image() {
    let vm = TestHostBackend::MacVm;
    for kind in [TestKind::Macos, TestKind::IosSimulator] {
        let linux = setup(Fake::linux(), kind, vm).await;
        assert_eq!(linux, "macOS VMs need a Mac with Apple silicon.");
        let intel = setup(Fake::new("macos", "x86_64"), kind, vm).await;
        assert_eq!(intel, linux);
    }
    let old = Fake::mac().answer("sw_vers -productVersion", "13.6.1");
    let old = setup(old, TestKind::Macos, vm).await;
    assert_eq!(
        old,
        "macOS VMs need macOS 14 or later, and this Mac runs 13.6.1."
    );
    let no_xcode = setup(Fake::mac(), TestKind::Macos, vm).await;
    assert_eq!(no_xcode, "Xcode command line tools were not found.");
    let no_base = setup(Fake::mac().program("xcrun"), TestKind::Macos, vm).await;
    assert_eq!(
        no_base,
        "The macOS test VM is built from a macOS restore image. Download the one for \
         this Mac's macOS and save it as /data/test-hosts/vm/restore.ipsw."
    );
    let base = || PathBuf::from("/data/test-hosts/vm/base");
    let restore = Fake::mac()
        .program("xcrun")
        .file("/data/test-hosts/vm/restore.ipsw");
    assert_eq!(
        found(restore, TestKind::Macos, vm).await,
        Ok(Ready::MacVm {
            base: base(),
            restore: Some("/data/test-hosts/vm/restore.ipsw".into()),
        })
    );
    // A build under way, or done, needs the restore image no more.
    for made in [
        "/data/test-hosts/vm/build.json",
        "/data/test-hosts/vm/base/disk.img",
    ] {
        let ready = found(Fake::mac().program("xcrun").file(made), TestKind::Macos, vm).await;
        assert_eq!(
            ready,
            Ok(Ready::MacVm {
                base: base(),
                restore: None,
            })
        );
    }
}

#[tokio::test]
async fn linux_containers_need_incus_answering_on_linux() {
    let incus = |fake| setup(fake, TestKind::Linux, TestHostBackend::IncusTest);
    assert_eq!(
        incus(Fake::mac()).await,
        "Linux test containers need a Linux device with Incus."
    );
    let missing = incus(Fake::linux()).await;
    assert!(
        missing.starts_with("Linux test containers need Incus"),
        "{missing}"
    );
    let mut silent = Fake::linux().file("/var/lib/incus/unix.socket");
    silent.incus = Err("connection refused".into());
    let silent = incus(silent).await;
    assert!(
        silent.starts_with("Incus doesn't answer on this device: connection refused."),
        "{silent}"
    );
    let answering = Fake::linux().file("/var/lib/incus/unix.socket");
    let ready = found(answering, TestKind::Linux, TestHostBackend::IncusTest).await;
    assert_eq!(ready, Ok(Ready::IncusTest));
}

#[tokio::test]
async fn one_round_asks_each_shared_question_once() {
    let root = format!("{HOME}/Library/Android/sdk");
    let fake = with_sdk(Fake::mac(), &root).xcode(RUNTIMES).node("22.0.0");
    let detector = Detector::new(fake, paths());
    let found = detector.detect(&TestKind::ALL).await;
    let backends: Vec<_> = found.iter().map(|f| (f.kind, f.backend)).collect();
    assert_eq!(
        backends,
        [
            (TestKind::IosSimulator, TestHostBackend::MacVm),
            (TestKind::IosSimulator, TestHostBackend::MacHost),
            (TestKind::AndroidEmulator, TestHostBackend::HostEmulator),
            (TestKind::Linux, TestHostBackend::IncusTest),
            (TestKind::Macos, TestHostBackend::MacVm),
        ]
    );
    let ran = detector.system.ran();
    assert_eq!(
        ran,
        [
            "/usr/bin/sw_vers -productVersion",
            "/usr/bin/xcrun simctl list runtimes --json",
            "/usr/bin/node --version",
        ]
    );
}

#[test]
fn a_finding_reads_as_its_host_state() {
    let device = DeviceId::derive("devbox");
    let ready = Finding {
        kind: TestKind::Linux,
        backend: TestHostBackend::IncusTest,
        found: Ok(Ready::IncusTest),
    };
    assert_eq!(ready.setup(), None);
    assert_eq!(ready.setup(), None);
    assert_eq!(
        ready.host(device),
        leases::host_id(device, TestKind::Linux, TestHostBackend::IncusTest)
    );
    let missing = Finding {
        found: Err("Something is missing.".into()),
        ..ready
    };
    assert_eq!(missing.setup().as_deref(), Some("Something is missing."));
}

#[tokio::test]
async fn a_check_looks_only_at_the_kinds_asked_about() {
    let detector = Detector::new(Fake::linux(), paths());
    assert_eq!(detector.check(&[]).await, []);

    let found = detector.check(&[TestKind::Linux]).await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, TestKind::Linux);
    assert!(found[0].found.is_err());
}

#[tokio::test]
async fn the_store_shows_what_the_detector_finds_for_its_hosts() {
    let hub = hub();
    let mut settings = crate::hub::default_settings(hub.device());
    settings.test_hosts.kinds = vec![TestKind::Linux];
    hub.apply([Change::Upsert {
        value: Entity::DeviceSettings(settings),
    }]);
    let backend = Arc::new(FakeBackend::new(
        TestKind::Linux,
        TestHostBackend::IncusTest,
    ));
    let store = TestHosts::open(
        hub.clone(),
        Db::open_in_memory().expect("a database"),
        Arc::new(FakeClock::new()),
        vec![backend],
    );
    let detector = Detector::new(Fake::linux(), paths());

    let setup = detector.detect(&[TestKind::Linux]).await[0].setup();
    let found = store.detect_with(&detector).await;
    assert!(setup.is_some());
    store.found(found);
    let id = host_id(hub.device().id, TestKind::Linux, TestHostBackend::IncusTest);
    match hub.get(&EntityKey::TestHost(id)) {
        Some(Entity::TestHost(host)) => {
            assert_eq!(host.setup, setup);
            assert_eq!(host.state, TestHostState::Unavailable);
        }
        other => panic!("no published host: {other:?}"),
    }
}
