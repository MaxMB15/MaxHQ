// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};

use maxhq_protocol::ids::LeaseId;

use super::*;
use crate::leases::detect::SystemImage;
use crate::session::clock::FakeClock;
use crate::testing::TempDir;

const PACKAGE: &str = "system-images;android-35;google_apis;arm64-v8a";

/// What the fake SDK answers, and what it was asked.
struct Script {
    ready: Result<Ready, String>,
    /// How many `getprop` answers say it's still booting.
    booting: usize,
    /// Whether the emulator quits as soon as it starts.
    quits: bool,
    /// Whether `emu kill` leaves it running.
    deaf: bool,
    busy: Vec<u16>,
    /// What `emu avd name` answers.
    avd: String,
    /// Whether `adb devices` lists the emulator.
    listed: bool,
    input: Option<String>,
    env: Vec<(String, String)>,
    log: Vec<String>,
    processes: Vec<Arc<FakeProcess>>,
}

#[derive(Default)]
struct FakeProcess {
    exited: AtomicBool,
    killed: AtomicBool,
}

impl Process for FakeProcess {
    fn exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    fn kill(&self) {
        self.killed.store(true, Ordering::SeqCst);
        self.exited.store(true, Ordering::SeqCst);
    }
}

#[derive(Clone)]
struct FakeAndroid(Arc<Mutex<Script>>);

impl FakeAndroid {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Script {
            ready: Ok(Ready::HostEmulator {
                sdk: AndroidSdk {
                    root: PathBuf::from("/sdk"),
                },
                image: SystemImage {
                    package: PACKAGE.into(),
                    api: 35,
                },
                node: PathBuf::from("/node/bin/node"),
            }),
            booting: 0,
            quits: false,
            deaf: false,
            busy: Vec::new(),
            avd: AVD.into(),
            listed: false,
            input: None,
            env: Vec::new(),
            log: Vec::new(),
            processes: Vec::new(),
        })))
    }

    fn script(&self) -> MutexGuard<'_, Script> {
        lock(&self.0)
    }

    fn note(&self, line: String) {
        self.script().log.push(line);
    }

    fn log(&self) -> Vec<String> {
        self.script().log.clone()
    }

    fn last(&self) -> Arc<FakeProcess> {
        self.script().processes.last().unwrap().clone()
    }
}

fn line(name: &str, command: &SdkCommand) -> String {
    format!(
        "{name} {} {}",
        program_name(&command.program),
        command.args.join(" ")
    )
}

impl Android for FakeAndroid {
    fn ready(&self) -> BoxFuture<'_, Result<Ready, String>> {
        let ready = self.script().ready.clone();
        async move { ready }.boxed()
    }

    fn java_home(&self) -> Option<PathBuf> {
        Some(PathBuf::from("/jbr"))
    }

    fn run<'a>(
        &'a self,
        command: &'a SdkCommand,
        input: Option<&'a str>,
        _timeout: Duration,
    ) -> BoxFuture<'a, Result<String, String>> {
        self.note(line("run", command));
        let mut script = self.script();
        script.env = command.env.clone();
        if let Some(input) = input {
            script.input = Some(input.to_owned());
        }
        let args: Vec<&str> = command.args.iter().map(String::as_str).collect();
        let said = match args.as_slice() {
            [.., "getprop", "sys.boot_completed"] => {
                let booted = script.booting == 0;
                script.booting = script.booting.saturating_sub(1);
                Ok(if booted { "1" } else { "" }.to_owned())
            }
            [.., "snapshot", "save", name] => {
                let avd = &command.env.iter().find(|(k, _)| k == "ANDROID_AVD_HOME");
                let avd = PathBuf::from(&avd.unwrap().1);
                std::fs::create_dir_all(avd.join(format!("{AVD}.avd/snapshots/{name}"))).unwrap();
                Ok("OK".to_owned())
            }
            [.., "emu", "kill"] => {
                if !script.deaf
                    && let Some(process) = script.processes.last()
                {
                    process.exited.store(true, Ordering::SeqCst);
                }
                Ok("OK".to_owned())
            }
            [.., "avd", "name"] => Ok(format!("{}\nOK", script.avd)),
            ["devices"] => Ok(if script.listed {
                "List of devices attached\nemulator-5680\tdevice".to_owned()
            } else {
                "List of devices attached".to_owned()
            }),
            _ => Ok(String::new()),
        };
        async move { said }.boxed()
    }

    fn boot(&self, command: &SdkCommand, _log: &Path) -> Result<Arc<dyn Process>, String> {
        self.note(line("boot", command));
        let process = Arc::new(FakeProcess::default());
        let mut script = self.script();
        process.exited.store(script.quits, Ordering::SeqCst);
        script.processes.push(process.clone());
        Ok(process)
    }

    fn port_free(&self, port: u16) -> bool {
        !self.script().busy.contains(&port)
    }

    fn agent_device(&self, config: Config) -> Arc<dyn Drive> {
        self.note(format!("agent-device {}", config.node.display()));
        Arc::new(FakeDrive(self.clone()))
    }
}

struct FakeDrive(FakeAndroid);

impl FakeDrive {
    fn note(&self, what: &str, target: &Target, session: &str, rest: &str) {
        assert_eq!(target.platform, Platform::Android);
        self.0.note(
            format!("drive {what} {} {session} {rest}", target.id)
                .trim_end()
                .to_owned(),
        );
    }
}

impl Drive for FakeDrive {
    fn install(&self) -> BoxFuture<'_, Result<(), String>> {
        self.0.note("drive install".into());
        async { Ok(()) }.boxed()
    }

    fn version(&self) -> &'static str {
        "0.0.0-fake"
    }

    fn run<'a>(
        &'a self,
        target: &'a Target,
        session: &'a str,
        command: &'a [String],
        _timeout: Duration,
    ) -> BoxFuture<'a, Result<RunOutput, String>> {
        self.note("run", target, session, &command.join(" "));
        async {
            Ok(RunOutput {
                exit_code: Some(0),
                stdout: "ran".into(),
                stderr: String::new(),
                truncated: false,
                timed_out: false,
            })
        }
        .boxed()
    }

    fn screenshot<'a>(
        &'a self,
        target: &'a Target,
        session: &'a str,
    ) -> BoxFuture<'a, Result<Vec<u8>, String>> {
        self.note("screenshot", target, session, "");
        async { Ok(b"PNG".to_vec()) }.boxed()
    }

    fn install_app<'a>(
        &'a self,
        target: &'a Target,
        session: &'a str,
        app: &'a Path,
    ) -> BoxFuture<'a, Result<(), String>> {
        self.note("install", target, session, &app.display().to_string());
        async { Ok(()) }.boxed()
    }

    fn close<'a>(
        &'a self,
        target: &'a Target,
        session: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        self.note("close", target, session, "");
        async { Err("agent-device's close failed".to_owned()) }.boxed()
    }

    fn stop(&self) -> BoxFuture<'_, ()> {
        self.0.note("drive stop".into());
        async {}.boxed()
    }
}

struct Fixture {
    dir: TempDir,
    android: FakeAndroid,
    clock: Arc<FakeClock>,
    emulator: Arc<Emulator<FakeAndroid>>,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new();
        let android = FakeAndroid::new();
        let clock = Arc::new(FakeClock::new());
        let emulator = Arc::new(Emulator::new(
            android.clone(),
            dir.path().to_owned(),
            clock.clone(),
        ));
        Self {
            dir,
            android,
            clock,
            emulator,
        }
    }

    /// `maxd` started again on the same data folder.
    fn restarted(&self) -> Arc<Emulator<FakeAndroid>> {
        Arc::new(Emulator::new(
            self.android.clone(),
            self.dir.path().to_owned(),
            self.clock.clone(),
        ))
    }

    fn avd_home(&self) -> PathBuf {
        self.dir.path().join("test-hosts").join("avd")
    }

    /// Runs `work` to its end, moving the clock on whenever it sleeps.
    async fn run<T: Send + 'static>(&self, work: impl Future<Output = T> + Send + 'static) -> T {
        let task = tokio::spawn(work);
        for _ in 0..5000 {
            if task.is_finished() {
                return task.await.unwrap();
            }
            if self.clock.sleepers() > 0 {
                self.clock.advance(Duration::from_secs(1));
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!("it never finished");
    }

    async fn build(&self) -> Result<Built, String> {
        let emulator = self.emulator.clone();
        self.run(async move { emulator.build().await }).await
    }

    async fn start(&self, emulator: &Arc<Emulator<FakeAndroid>>) -> Result<String, String> {
        let emulator = emulator.clone();
        self.run(async move { emulator.start(LeaseId::generate()).await })
            .await
    }

    fn clear(&self) {
        self.android.script().log.clear();
    }
}

#[tokio::test]
async fn a_build_makes_the_avd_in_maxds_folder_and_saves_a_clean_snapshot() {
    let fx = Fixture::new();
    let built = fx.build().await.unwrap();
    assert_eq!(built.agent_device.as_deref(), Some("0.0.0-fake"));
    let avd = fx.avd_home().join("mhq-test.avd");
    assert_eq!(
        fx.android.log(),
        [
            "agent-device /node/bin/node".to_owned(),
            format!(
                "run avdmanager create avd --name mhq-test --package {PACKAGE} --path {} --force",
                avd.display()
            ),
            "drive install".to_owned(),
            "boot emulator -avd mhq-test -port 5680 -no-window -no-audio -no-boot-anim \
             -no-snapshot-save -no-snapshot-load"
                .to_owned(),
            "run adb -s emulator-5680 shell getprop sys.boot_completed".to_owned(),
            "run adb -s emulator-5680 emu avd snapshot save clean".to_owned(),
            "run adb -s emulator-5680 emu kill".to_owned(),
        ]
    );
    let script = fx.android.script();
    assert_eq!(script.input.as_deref(), Some("no\n"));
    let home = fx.avd_home().display().to_string();
    for (name, value) in [
        ("ANDROID_HOME", "/sdk"),
        ("ANDROID_SDK_ROOT", "/sdk"),
        ("ANDROID_AVD_HOME", home.as_str()),
        ("JAVA_HOME", "/jbr"),
    ] {
        assert!(
            script.env.contains(&(name.to_owned(), value.to_owned())),
            "{name}"
        );
    }
    assert!(avd.join("snapshots").join("clean").is_dir());
    assert!(!fx.dir.path().join("test-hosts/emulator-serial").exists());
}

#[tokio::test]
async fn a_build_or_a_check_answers_what_is_missing() {
    let fx = Fixture::new();
    fx.android.script().ready = Err("Android SDK was not found.".into());
    assert_eq!(
        fx.emulator.detect().await.as_deref(),
        Some("Android SDK was not found.")
    );
    assert_eq!(fx.build().await.unwrap_err(), "Android SDK was not found.");
    assert!(fx.android.log().is_empty());

    fx.android.script().ready = FakeAndroid::new().script().ready.clone();
    assert_eq!(fx.emulator.detect().await, None);
}

#[tokio::test]
async fn the_boot_waits_on_the_clock_until_android_has_started() {
    let fx = Fixture::new();
    fx.android.script().booting = 3;
    fx.build().await.unwrap();
    let polls = fx
        .android
        .log()
        .iter()
        .filter(|line| line.ends_with("sys.boot_completed"))
        .count();
    assert_eq!(polls, 4);
}

#[tokio::test]
async fn a_boot_that_never_finishes_fails_and_stops_the_emulator() {
    let fx = Fixture::new();
    fx.android.script().booting = usize::MAX;
    let error = fx.build().await.unwrap_err();
    assert_eq!(
        error,
        "Android didn't finish starting in the emulator within 10 minutes."
    );
    assert!(fx.android.last().exited());
    assert!(fx
        .android
        .log()
        .contains(&"run adb -s emulator-5680 emu kill".to_owned()));
    assert!(!fx.avd_home().join("mhq-test.avd/snapshots/clean").exists());
}

#[tokio::test]
async fn an_emulator_that_quits_while_booting_says_where_its_log_is() {
    let fx = Fixture::new();
    fx.android.script().quits = true;
    let error = fx.build().await.unwrap_err();
    assert!(
        error.starts_with("The emulator stopped before Android started."),
        "{error}"
    );
    assert!(error.contains("emulator.log"), "{error}");
}

#[tokio::test]
async fn a_lease_boots_from_the_clean_snapshot_without_saving_on_a_free_port() {
    let fx = Fixture::new();
    fx.build().await.unwrap();
    fx.android.script().busy = vec![5681];
    fx.clear();
    let serial = fx.start(&fx.emulator).await.unwrap();
    assert_eq!(serial, "emulator-5678");
    assert_eq!(
        fx.android.log(),
        [
            "boot emulator -avd mhq-test -port 5678 -no-window -no-audio -no-boot-anim \
             -no-snapshot-save -snapshot clean",
            "run adb -s emulator-5678 shell getprop sys.boot_completed",
        ]
    );
    let written = std::fs::read_to_string(fx.dir.path().join("test-hosts/emulator-serial"));
    assert_eq!(written.unwrap(), "emulator-5678");
}

#[tokio::test]
async fn a_lease_needs_the_clean_snapshot() {
    let fx = Fixture::new();
    let error = fx.start(&fx.emulator).await.unwrap_err();
    assert_eq!(
        error,
        "The emulator's clean snapshot is gone, so build the host again."
    );
    assert!(!fx.android.log().iter().any(|line| line.starts_with("boot")));
}

#[tokio::test]
async fn lease_calls_drive_the_serial_through_agent_device() {
    let fx = Fixture::new();
    fx.build().await.unwrap();
    let lease = LeaseId::generate();
    let on = LeaseTarget {
        lease,
        target: fx.start(&fx.emulator).await.unwrap(),
    };
    fx.clear();
    let session = agent_device::session(lease);
    let command = ["snapshot".to_owned(), "-i".to_owned()];
    let ran = fx.emulator.run(&on, &command, Duration::from_secs(5)).await;
    assert_eq!(ran.unwrap().stdout, "ran");
    assert_eq!(fx.emulator.screenshot(&on).await.unwrap(), b"PNG");
    let installed = fx
        .emulator
        .install(&on, PathBuf::from("/lease/app.apk"))
        .await
        .unwrap();
    assert_eq!(installed, "Installed app.apk on emulator-5680.");
    assert_eq!(
        fx.android.log(),
        [
            format!("drive run emulator-5680 {session} snapshot -i"),
            format!("drive screenshot emulator-5680 {session}"),
            format!("drive install emulator-5680 {session} /lease/app.apk"),
        ]
    );
}

#[tokio::test]
async fn closing_a_lease_ends_its_session_then_stops_the_emulator_by_serial() {
    let fx = Fixture::new();
    fx.build().await.unwrap();
    let lease = LeaseId::generate();
    let on = LeaseTarget {
        lease,
        target: fx.start(&fx.emulator).await.unwrap(),
    };
    fx.android.script().deaf = true;
    fx.clear();
    let emulator = fx.emulator.clone();
    // The session's close fails, and the stop goes on regardless.
    fx.run(async move { emulator.close(&on).await })
        .await
        .unwrap();
    assert_eq!(
        fx.android.log(),
        [
            format!("drive close emulator-5680 {}", agent_device::session(lease)),
            "run adb -s emulator-5680 emu kill".to_owned(),
        ]
    );
    // It didn't quit when asked, so it was killed.
    assert!(fx.android.last().killed.load(Ordering::SeqCst));
    assert!(!fx.dir.path().join("test-hosts/emulator-serial").exists());
}

#[tokio::test]
async fn a_reset_after_a_restart_stops_the_emulator_maxd_started() {
    let fx = Fixture::new();
    fx.build().await.unwrap();
    fx.start(&fx.emulator).await.unwrap();
    fx.clear();
    let restarted = fx.restarted();
    let reset = restarted.clone();
    fx.run(async move { reset.reset().await }).await.unwrap();
    assert_eq!(
        fx.android.log(),
        [
            "agent-device /node/bin/node",
            "run adb -s emulator-5680 emu avd name",
            "run adb -s emulator-5680 emu kill",
            "run adb devices",
            "drive stop",
        ]
    );
    assert!(!fx.dir.path().join("test-hosts/emulator-serial").exists());
}

#[tokio::test]
async fn a_reset_leaves_an_emulator_maxd_did_not_start_alone() {
    let fx = Fixture::new();
    fx.build().await.unwrap();
    fx.start(&fx.emulator).await.unwrap();
    // Since then the serial went to an emulator of yours.
    fx.android.script().avd = "Pixel_8".into();
    fx.clear();
    let restarted = fx.restarted();
    let reset = restarted.clone();
    fx.run(async move { reset.reset().await }).await.unwrap();
    assert!(!fx
        .android
        .log()
        .iter()
        .any(|line| line.ends_with("emu kill")));
    assert!(!fx.dir.path().join("test-hosts/emulator-serial").exists());
}

#[tokio::test]
async fn a_reset_fails_when_a_restarted_emulator_will_not_stop() {
    let fx = Fixture::new();
    fx.build().await.unwrap();
    fx.start(&fx.emulator).await.unwrap();
    {
        let mut script = fx.android.script();
        script.deaf = true;
        script.listed = true;
    }
    let restarted = fx.restarted();
    let reset = restarted.clone();
    let error = fx
        .run(async move { reset.reset().await })
        .await
        .unwrap_err();
    assert_eq!(error, "The emulator emulator-5680 didn't stop.");
}
