// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use maxhq_protocol::agents::ToolImage;
use maxhq_protocol::guest::HostToGuest;

use super::*;

/// What the fakes were asked, in order.
type Log = Arc<Mutex<Vec<String>>>;

fn note(log: &Log, call: String) {
    lock(log).push(call);
}

/// VMs that only note what they're asked, and make the bundle files a run
/// would.
struct FakeVms {
    log: Log,
    /// Fails the next start.
    fail_start: Mutex<bool>,
}

impl Vms for FakeVms {
    fn install<'a>(
        &'a self,
        restore_image: &'a Path,
        bundle: &'a Path,
    ) -> BoxFuture<'a, Result<(), String>> {
        note(&self.log, format!("install {}", restore_image.display()));
        std::fs::create_dir_all(bundle).unwrap();
        std::fs::write(bundle.join(bundle::DISK), vec![1; 4096]).unwrap();
        let shape = serde_json::to_vec(&SHAPE).unwrap();
        std::fs::write(bundle.join(bundle::SHAPE), shape).unwrap();
        async { Ok(()) }.boxed()
    }

    fn start(&self, start: Op) -> BoxFuture<'_, Result<Box<dyn Vm>, String>> {
        let Op::Start {
            bundle,
            share,
            restore,
            window,
            note: shown,
            ..
        } = start
        else {
            panic!("not a start");
        };
        let name = bundle.file_name().unwrap().to_string_lossy().into_owned();
        let lease = std::fs::read(share.path.join(LEASE_FILE)).unwrap();
        let lease: LeaseFile = serde_json::from_slice(&lease).unwrap();
        assert!(share.path.join(MAXD_FILE).exists());
        note(
            &self.log,
            format!(
                "start {name} restore={restore} window={window} note={} {}",
                shown.is_some(),
                lease.instance
            ),
        );
        let failed = std::mem::take(&mut *lock(&self.fail_start));
        let vm = FakeVm {
            log: self.log.clone(),
            bundle,
        };
        async move {
            if failed {
                return Err("no room".to_owned());
            }
            Ok(Box::new(vm) as Box<dyn Vm>)
        }
        .boxed()
    }
}

struct FakeVm {
    log: Log,
    bundle: PathBuf,
}

impl Vm for FakeVm {
    fn save(&self) -> BoxFuture<'_, Result<(), String>> {
        note(&self.log, "save".into());
        std::fs::write(self.bundle.join(bundle::STATE), b"state").unwrap();
        async { Ok(()) }.boxed()
    }

    fn stop(&self) -> BoxFuture<'_, ()> {
        note(&self.log, "stop".into());
        async {}.boxed()
    }
}

/// Guest links whose guest answers every request well, unless told to
/// fail its runs or its link.
struct FakeLinks {
    log: Log,
    fail_up: Mutex<bool>,
    /// What a failing run prints, if runs fail.
    fail_run: Mutex<Option<String>>,
}

impl Links for FakeLinks {
    fn up<'a>(
        &'a self,
        _workspace: WorkspaceId,
        instance: &'a InstanceName,
        wait: Duration,
    ) -> BoxFuture<'a, Result<(), String>> {
        note(&self.log, format!("up {instance} {}s", wait.as_secs()));
        let failed = std::mem::take(&mut *lock(&self.fail_up));
        async move {
            if failed {
                return Err("timed out".to_owned());
            }
            Ok(())
        }
        .boxed()
    }

    fn down(&self, _workspace: WorkspaceId) {
        note(&self.log, "down".into());
    }

    fn ask(
        &self,
        _workspace: WorkspaceId,
        _wait: Duration,
        make: MakeRequest,
    ) -> BoxFuture<'_, Result<GuestToHost, String>> {
        let failed = lock(&self.fail_run).clone();
        let answer = match make(7) {
            HostToGuest::LeaseRun { req, command, .. } => {
                let script = command.last().cloned().unwrap_or_default();
                let first = script.lines().find(|line| *line != "set -e").unwrap_or("");
                note(&self.log, format!("run {first}"));
                GuestToHost::LeaseRan {
                    req,
                    result: Ok(RunOutput {
                        exit_code: Some(if failed.is_some() { 1 } else { 0 }),
                        stdout: String::new(),
                        stderr: failed.unwrap_or_default(),
                        truncated: false,
                        timed_out: false,
                    }),
                }
            }
            HostToGuest::LeaseScreenshot { req, .. } => {
                note(&self.log, "screenshot".into());
                GuestToHost::LeaseScreen {
                    req,
                    result: Ok(ToolImage {
                        mime: "image/png".into(),
                        data: "UE5H".into(),
                    }),
                }
            }
            HostToGuest::LeaseInstall { req, path, .. } => {
                note(&self.log, format!("install {path}"));
                GuestToHost::LeaseDone {
                    req,
                    result: Ok(()),
                }
            }
            HostToGuest::LeaseClose { req, .. } => {
                note(&self.log, "close".into());
                GuestToHost::LeaseDone {
                    req,
                    result: Ok(()),
                }
            }
            other => panic!("unexpected {other:?}"),
        };
        async move { Ok(answer) }.boxed()
    }
}

struct FakeMac {
    root: PathBuf,
}

impl Mac for FakeMac {
    fn xcode(&self) -> BoxFuture<'_, Result<PathBuf, String>> {
        let xcode = self.root.join("Xcode.app");
        std::fs::create_dir_all(xcode.join("Contents")).unwrap();
        async move { Ok(xcode) }.boxed()
    }

    fn ios_runtime(&self) -> BoxFuture<'_, Result<PathBuf, String>> {
        let image = self.root.join("runtime.dmg");
        std::fs::write(&image, b"dmg").unwrap();
        async move { Ok(image) }.boxed()
    }
}

struct Setup {
    _dir: crate::testing::TempDir,
    folder: PathBuf,
    log: Log,
    vms: Arc<FakeVms>,
    links: Arc<FakeLinks>,
    macos: MacVm,
    ios: MacVm,
}

fn setup() -> Setup {
    let dir = crate::testing::TempDir::new();
    let folder = dir.path().join("vm");
    let maxd = dir.path().join("maxd");
    std::fs::write(&maxd, b"maxd").unwrap();
    let log = Log::default();
    let vms = Arc::new(FakeVms {
        log: log.clone(),
        fail_start: Mutex::new(false),
    });
    let links = Arc::new(FakeLinks {
        log: log.clone(),
        fail_up: Mutex::new(false),
        fail_run: Mutex::new(None),
    });
    let restore = dir.path().join("restore.ipsw");
    let find: Find = Arc::new(move || {
        let restore = restore.clone();
        async move {
            Ok(Ready::MacVm {
                base: PathBuf::new(),
                restore: Some(restore),
            })
        }
        .boxed()
    });
    let shared = Arc::new(Shared {
        folder: folder.clone(),
        runtime: dir.path().join("run"),
        maxd,
        vms: vms.clone(),
        links: links.clone(),
        mac: Arc::new(FakeMac {
            root: dir.path().join("mac"),
        }),
        find,
        building: tokio::sync::Mutex::new(()),
    });
    Setup {
        folder,
        log,
        vms,
        links,
        macos: MacVm::new(TestKind::Macos, shared.clone()),
        ios: MacVm::new(TestKind::IosSimulator, shared),
        _dir: dir,
    }
}

impl Setup {
    /// What the fakes were asked since the last call.
    fn calls(&self) -> Vec<String> {
        std::mem::take(&mut *lock(&self.log))
    }

    /// A built base, as a build leaves it.
    fn base(&self) {
        let base = self.folder.join(BASE);
        std::fs::create_dir_all(&base).unwrap();
        for file in [bundle::DISK, bundle::STATE] {
            std::fs::write(base.join(file), b"x").unwrap();
        }
        let shape = serde_json::to_vec(&SHAPE).unwrap();
        std::fs::write(base.join(bundle::SHAPE), shape).unwrap();
    }
}

fn target(name: &str) -> LeaseTarget {
    LeaseTarget {
        lease: LeaseId::derive(name),
        target: String::new(),
    }
}

#[test]
fn the_setup_window_says_what_happens_to_the_password() {
    assert!(SETUP_NOTE.contains("a password you use nowhere else"));
    assert!(SETUP_NOTE.contains("anything running in the VM can read it"));
}

#[tokio::test]
async fn a_lease_resumes_a_clone_of_the_base_and_deletes_it_at_the_end() {
    let s = setup();
    let on = target("lease");
    let refused = s.macos.start(on.lease).await.unwrap_err();
    assert_eq!(refused, "The macOS test VM isn't built yet.");
    s.base();
    let instance = s.macos.start(on.lease).await.unwrap();
    assert!(instance.starts_with("mhq-vm-"), "{instance}");
    let clone = s
        .folder
        .join(LEASES)
        .join("macos")
        .join(on.lease.to_string());
    assert!(clone.join(bundle::STATE).exists());
    let lease = std::fs::read(clone.join(SHARE).join(LEASE_FILE)).unwrap();
    let lease: LeaseFile = serde_json::from_slice(&lease).unwrap();
    assert_eq!(lease.kind, TestKind::Macos);
    assert_eq!(lease.instance, instance);
    let name = on.lease.to_string();
    assert_eq!(
        s.calls(),
        [
            format!("start {name} restore=true window=false note=false {instance}"),
            format!("up {instance} 180s"),
        ]
    );

    let command = ["echo".to_owned(), "hi".to_owned()];
    let ran = s.macos.run(&on, &command, Duration::from_secs(5)).await;
    assert_eq!(ran.unwrap().exit_code, Some(0));
    assert_eq!(s.macos.screenshot(&on).await.unwrap(), b"PNG");
    s.macos.close(&on).await.unwrap();
    assert!(!clone.exists());
    assert_eq!(s.calls(), ["run hi", "screenshot", "close", "down", "stop"]);
    let gone = s.macos.run(&on, &command, Duration::from_secs(5)).await;
    assert_eq!(gone.unwrap_err(), "That lease's VM isn't running.");
    assert!(s.folder.join(BASE).join(bundle::STATE).exists());
}

#[tokio::test]
async fn a_guest_that_never_links_stops_its_vm_and_its_clone_goes() {
    let s = setup();
    s.base();
    let on = target("lease");
    *lock(&s.links.fail_up) = true;
    let failed = s.ios.start(on.lease).await.unwrap_err();
    assert_eq!(failed, "The VM's guest didn't link: timed out");
    let calls = s.calls();
    assert_eq!(calls[2..], ["down", "stop"]);
    let clone = s
        .folder
        .join(LEASES)
        .join("ios_simulator")
        .join(on.lease.to_string());
    assert!(!clone.exists());

    *lock(&s.vms.fail_start) = true;
    assert_eq!(s.ios.start(on.lease).await.unwrap_err(), "no room");
    assert!(!clone.exists());
}

#[tokio::test]
async fn an_upload_goes_through_the_shared_folder() {
    let s = setup();
    s.base();
    let on = target("lease");
    s.ios.start(on.lease).await.unwrap();
    s.calls();
    let app = s.folder.parent().unwrap().join("Demo.app");
    std::fs::create_dir_all(&app).unwrap();
    let said = s.ios.install(&on, app).await.unwrap();
    assert_eq!(said, "Installed Demo.app on the Simulator.");
    let clone = s
        .folder
        .join(LEASES)
        .join("ios_simulator")
        .join(on.lease.to_string());
    assert!(clone
        .join(SHARE)
        .join(UPLOADS)
        .join("0")
        .join("Demo.app")
        .is_dir());
    assert_eq!(names(&clone.join(STAGE)), Vec::<String>::new());
    assert_eq!(
        s.calls(),
        [format!("install {GUEST_SHARE}/{UPLOADS}/0/Demo.app")]
    );
    s.ios.reset().await.unwrap();
    assert!(!clone.exists());
    assert_eq!(s.calls(), ["down", "stop"]);
}

/// What's in `folder`, by name.
fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// Windows makes links only for an administrator.
#[cfg(unix)]
#[tokio::test]
async fn an_upload_never_follows_what_the_guest_planted() {
    use std::os::unix::fs::symlink;

    let s = setup();
    s.base();
    let on = target("lease");
    s.ios.start(on.lease).await.unwrap();
    s.calls();
    let root = s.folder.parent().unwrap().to_owned();
    let app = root.join("Demo.app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(app.join("binary"), b"app").unwrap();
    // A folder on the host the guest wants the upload written into.
    let outside = root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let share = s
        .folder
        .join(LEASES)
        .join("ios_simulator")
        .join(on.lease.to_string())
        .join(SHARE);
    let uploads = share.join(UPLOADS);

    // Links, a file and a full folder sit where the next uploads would go.
    std::fs::create_dir_all(uploads.join("1")).unwrap();
    std::fs::write(uploads.join("1").join("mine"), b"").unwrap();
    symlink(&outside, uploads.join("0")).unwrap();
    std::fs::write(uploads.join("2"), b"").unwrap();
    symlink(root.join("not-there"), uploads.join("3")).unwrap();
    s.ios.install(&on, app.clone()).await.unwrap();
    assert_eq!(names(&outside), Vec::<String>::new());
    assert_eq!(
        std::fs::read(uploads.join("4").join("Demo.app").join("binary")).unwrap(),
        b"app"
    );
    assert_eq!(
        s.calls(),
        [format!("install {GUEST_SHARE}/{UPLOADS}/4/Demo.app")]
    );

    // A link to the host in place of the uploads folder stops the upload.
    std::fs::rename(&uploads, share.join("moved")).unwrap();
    symlink(&outside, &uploads).unwrap();
    s.ios.install(&on, app).await.unwrap_err();
    assert_eq!(names(&outside), Vec::<String>::new());
    assert_eq!(s.calls(), Vec::<String>::new());
}

#[tokio::test]
async fn a_guest_that_takes_every_name_gets_no_upload() {
    let s = setup();
    s.base();
    let on = target("lease");
    s.ios.start(on.lease).await.unwrap();
    s.calls();
    let app = s.folder.parent().unwrap().join("Demo.app");
    std::fs::create_dir_all(&app).unwrap();
    let clone = s
        .folder
        .join(LEASES)
        .join("ios_simulator")
        .join(on.lease.to_string());
    let uploads = clone.join(SHARE).join(UPLOADS);
    std::fs::create_dir_all(&uploads).unwrap();
    for n in 0..=UPLOAD_TRIES {
        std::fs::write(uploads.join(n.to_string()), b"").unwrap();
    }
    let said = s.ios.install(&on, app).await.unwrap_err();
    assert_eq!(said, "The VM's guest keeps taking the upload's name.");
    assert_eq!(names(&clone.join(STAGE)), Vec::<String>::new());
    assert_eq!(s.calls(), Vec::<String>::new());
}

// Windows makes links only for an administrator.
#[cfg(unix)]
#[tokio::test]
async fn the_builds_writes_never_follow_what_the_guest_planted() {
    use std::os::unix::fs::symlink;

    let dir = crate::testing::TempDir::new();
    let share = dir.path().join("share");
    let stage = dir.path().join("stage");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&share).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let victim = outside.join("victim");
    std::fs::write(&victim, b"untouched").unwrap();

    // The lease file, written while the guest runs, and the setup script.
    let instance = instance("build");
    symlink(&victim, share.join(LEASE_FILE)).unwrap();
    write_lease_file(&share, TestKind::Macos, &instance).unwrap();
    symlink(&victim, share.join(SETUP_FILE)).unwrap();
    write_to_share(&share, SETUP_FILE, b"setup").unwrap();
    assert_eq!(std::fs::read(&victim).unwrap(), b"untouched");
    for name in [LEASE_FILE, SETUP_FILE] {
        let meta = std::fs::symlink_metadata(share.join(name)).unwrap();
        assert!(meta.is_file(), "{name}");
    }
    assert_eq!(std::fs::read(share.join(SETUP_FILE)).unwrap(), b"setup");

    // Xcode's copy, with a link where it goes.
    let xcode = dir.path().join("Xcode.app");
    std::fs::create_dir_all(&xcode).unwrap();
    std::fs::write(xcode.join("binary"), b"xcode").unwrap();
    symlink(&outside, share.join("Xcode.app")).unwrap();
    copy_to_share(&xcode, &stage, &share, "Xcode.app")
        .await
        .unwrap();
    assert_eq!(names(&outside), ["victim"]);
    assert_eq!(
        std::fs::read(share.join("Xcode.app").join("binary")).unwrap(),
        b"xcode"
    );

    // The runtime's image, a file, the same way.
    let image = dir.path().join("image");
    std::fs::write(&image, b"image").unwrap();
    symlink(&victim, share.join(RUNTIME_FILE)).unwrap();
    copy_to_share(&image, &stage, &share, RUNTIME_FILE)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&victim).unwrap(), b"untouched");
    assert_eq!(std::fs::read(share.join(RUNTIME_FILE)).unwrap(), b"image");
}

#[tokio::test]
async fn a_build_installs_waits_for_setup_brings_in_the_tools_and_saves() {
    let s = setup();
    let built = s.macos.build().await.unwrap();
    assert_eq!(built.agent_device.as_deref(), Some(agent_device::VERSION));
    assert_eq!(built.vm.unwrap().memory_bytes, SHAPE.memory_bytes);
    let restore = s.folder.parent().unwrap().join("restore.ipsw");
    assert_eq!(
        s.calls(),
        [
            format!("install {}", restore.display()),
            "start building restore=false window=true note=true mhq-vm-build".into(),
            "up mhq-vm-build 7200s".into(),
            "run share=\"/Volumes/My Shared Files\"".into(),
            "run sudo xcodebuild -license accept".into(),
            "run xcrun simctl runtime add \"/Volumes/My Shared Files/ios-runtime.dmg\"".into(),
            "run base=https://nodejs.org/dist/latest-v22.x".into(),
            "screenshot".into(),
            "save".into(),
            "down".into(),
            "stop".into(),
        ]
    );
    assert!(s.folder.join(BASE).join(bundle::STATE).exists());
    for gone in [BUILDING, BUILD_SHARE, BUILD_STAGE, STEP_FILE] {
        assert!(!s.folder.join(gone).exists(), "{gone}");
    }
    // The other kind shares the base, so its build finds it made.
    s.ios.build().await.unwrap();
    assert_eq!(s.calls(), Vec::<String>::new());
}

#[tokio::test]
async fn a_failed_step_keeps_the_build_to_pick_up_there() {
    let s = setup();
    *lock(&s.links.fail_run) = Some("no space left\n".into());
    let failed = s.macos.build().await.unwrap_err();
    assert_eq!(failed, "The VM's Xcode step failed: no space left");
    assert_eq!(read_step(&s.folder), Step::Xcode);
    assert!(s.folder.join(BUILDING).join(bundle::DISK).exists());
    assert!(!s.folder.join(BUILD_SHARE).join("Xcode.app").exists());
    let calls = s.calls();
    assert_eq!(calls[calls.len() - 2..], ["down", "stop"]);

    // The next build boots what's there, with no window, and goes on.
    *lock(&s.links.fail_run) = None;
    s.macos.build().await.unwrap();
    let calls = s.calls();
    assert_eq!(
        calls[..3],
        [
            "start building restore=false window=false note=false mhq-vm-build",
            "up mhq-vm-build 180s",
            "run share=\"/Volumes/My Shared Files\"",
        ]
    );
}

#[test]
fn the_setup_script_gives_sudo_logs_in_and_starts_the_guest() {
    let script = setup_script();
    assert!(script.starts_with("#!/bin/sh\n"));
    for line in [
        "share=\"/Volumes/My Shared Files\"",
        "sudo visudo -cf /etc/sudoers.d/maxhq >/dev/null",
        "sudo sysadminctl -autologin set -userName \"$user\" -password \"$password\"",
        "sudo pmset -a sleep 0 displaysleep 0 disksleep 0",
        "cp \"$share/maxd\" \"$HOME/.maxhq/bin/maxd\"",
        "<array><string>$HOME/.maxhq/bin/maxd</string><string>guest</string><string>--vm</string></array>",
        "launchctl bootstrap \"gui/$(id -u)\" \"$plist\"",
    ] {
        assert!(script.lines().any(|l| l.trim() == line), "{line}");
    }
    assert_eq!(SETUP_COMMAND, format!("sh \"{GUEST_SHARE}/{SETUP_FILE}\""));
}

#[test]
fn the_mac_gives_its_xcode_app_and_newest_ios_runtime_image() {
    let developer = Path::new("/Applications/Xcode-26.app/Contents/Developer");
    assert_eq!(
        xcode_app(developer),
        Some(PathBuf::from("/Applications/Xcode-26.app"))
    );
    assert_eq!(
        xcode_app(Path::new("/Library/Developer/CommandLineTools")),
        None
    );
    let listed = r#"{
        "A": {"path": "/rt/ios-18.dmg", "platformIdentifier": "com.apple.platform.iphonesimulator", "version": "18.6", "state": "Ready"},
        "B": {"path": "/rt/ios-26.dmg", "platformIdentifier": "com.apple.platform.iphonesimulator", "version": "26.1", "state": "Ready"},
        "C": {"path": "/rt/ios-27.dmg", "platformIdentifier": "com.apple.platform.iphonesimulator", "version": "27.0", "state": "Deleting"},
        "D": {"path": "/rt/watch.dmg", "platformIdentifier": "com.apple.platform.watchsimulator", "version": "30.0", "state": "Ready"}
    }"#;
    assert_eq!(newest_ios_image(listed), Some("/rt/ios-26.dmg".into()));
    assert_eq!(newest_ios_image("{}"), None);
}

#[test]
fn instance_names_are_valid_and_short() {
    let lease = LeaseId::derive("lease").to_string();
    let name = instance(&lease).to_string();
    assert_eq!(name.len(), "mhq-vm-".len() + 8);
    assert_eq!(instance("build").to_string(), "mhq-vm-build");
}

/// A helper that answers each request done, as `maxd-vm` does.
#[cfg(unix)]
const FAKE_HELPER: &str = r#"#!/bin/sh
while read -r line; do
  id="$(printf '%s' "$line" | sed 's/^{"id":\([0-9]*\).*/\1/')"
  case "$line" in
    *'"op":"start"'*) echo '{"event":"progress","fraction":0.5}' ;;
  esac
  echo "{\"event\":\"done\",\"id\":$id,\"version\":null}"
  case "$line" in *'"op":"stop"'*) exit 0 ;; esac
done
"#;

// The fake helper is an sh script.
#[cfg(unix)]
#[tokio::test]
async fn the_helper_process_answers_each_request_by_its_id() {
    let dir = crate::testing::TempDir::new();
    let binary = dir.path().join("maxd-vm");
    std::fs::write(&binary, FAKE_HELPER).unwrap();
    maxhq_os::fsmode::set_mode(&binary, 0o755).unwrap();
    let helper = Helper { binary };
    let vm = helper
        .start(Op::Start {
            bundle: dir.path().join("b"),
            share: Share {
                path: dir.path().join("s"),
                read_only: false,
            },
            socket: dir.path().join("v.sock"),
            restore: true,
            window: false,
            note: None,
        })
        .await
        .unwrap();
    vm.save().await.unwrap();
    vm.stop().await;

    let missing = Helper {
        binary: dir.path().join("none"),
    };
    let error = missing.start(Op::Stop).await.err().unwrap();
    assert!(error.starts_with("couldn't start "), "{error}");
}
