// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use super::*;
use crate::service::install;
use crate::service::tests::{module_unit, setup, Fake};
use crate::testing::TempDir;

const TARGET: &str = "gui/501/com.maxagenthq.maxd";
const NOTHING: &str = "maxd isn't installed, so there was nothing to stop or remove.";

/// Holders that end what they're told to.
#[derive(Default)]
struct FakeHolders {
    calls: usize,
    ending: Ending,
}

impl FakeHolders {
    fn ending(ended: usize, left: &[&str]) -> Self {
        Self {
            calls: 0,
            ending: Ending {
                ended,
                left: left.iter().map(|line| (*line).to_owned()).collect(),
            },
        }
    }
}

impl Holders for FakeHolders {
    fn end_all(&mut self) -> Ending {
        self.calls += 1;
        self.ending.clone()
    }
}

/// An uninstall from the command line that doesn't wait between checks.
fn quick(delete_data: bool) -> Uninstall {
    Uninstall {
        pause: Duration::ZERO,
        ..Uninstall::new(delete_data)
    }
}

/// Runs uninstall, and gives how it ended and what it said.
fn run(
    setup: &Setup,
    how: Uninstall,
    clis: &Clis,
    fake: &mut Fake,
    holders: &mut FakeHolders,
) -> (Result<(), ServiceError>, Vec<String>) {
    let mut said = Vec::new();
    let result = uninstall(setup, how, clis, fake, holders, &mut |line| said.push(line));
    (result, said)
}

/// What uninstall left, from its error.
fn problems(result: Result<(), ServiceError>) -> Vec<String> {
    match result {
        Err(ServiceError::Unfinished(problems)) => problems,
        other => panic!("expected an unfinished uninstall, got {other:?}"),
    }
}

/// The commands run, leaving out the looks for leftover processes.
fn without_pgrep(fake: &Fake) -> Vec<&str> {
    fake.ran
        .iter()
        .map(String::as_str)
        .filter(|line| !line.starts_with("pgrep"))
        .collect()
}

/// The commands run, leaving out the looks for leftover processes and at
/// whether the service runs.
fn changes(fake: &Fake) -> Vec<&str> {
    without_pgrep(fake)
        .into_iter()
        .filter(|line| {
            !line.starts_with("launchctl print") && !line.starts_with("systemctl --user is-active")
        })
        .collect()
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn uninstall_stops_the_service_ends_the_agents_and_removes_the_file() {
    let dir = TempDir::new();
    let setup = setup(&dir, Manager::Launchd);
    install(&setup, &mut Fake::default()).unwrap();
    // launchd knows the job until it's booted out, and nothing else runs.
    let mut fake = Fake::default()
        .answer_times("launchctl print", "", 1)
        .fail("pgrep", "");
    let mut holders = FakeHolders::ending(2, &[]);
    let (result, said) = run(
        &setup,
        quick(false),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    result.unwrap();

    let file = setup.file();
    assert_eq!(
        said,
        [
            "Stopped the maxd service.".to_owned(),
            "Ended 2 running agents.".to_owned(),
            format!("Removed {}.", file.display()),
        ]
    );
    assert!(!file.exists());
    assert_eq!(holders.calls, 1);
    assert_eq!(
        fake.ran[..4],
        [
            "pgrep -U 501 -f -- maxd update watch --build ".to_owned(),
            format!("launchctl print {TARGET}"),
            format!("launchctl bootout {TARGET}"),
            format!("launchctl print {TARGET}"),
        ]
    );
    assert!(fake
        .ran
        .contains(&"pgrep -U 501 -P 1 -x -- maxd-vm".to_owned()));
    assert!(
        dir.path().join("home/Library/Logs/MaxHQ").is_dir(),
        "logs stay"
    );

    // A second run finds nothing to do and doesn't fail.
    let mut fake = Fake::default().fail("pgrep", "");
    let mut holders = FakeHolders::default();
    let (result, said) = run(
        &setup,
        quick(false),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    result.unwrap();
    assert_eq!(said, [NOTHING]);
}

#[test]
fn uninstall_disables_the_user_unit_and_reloads_systemd() {
    let dir = TempDir::new();
    let setup = setup(&dir, Manager::Systemd);
    install(&setup, &mut Fake::default()).unwrap();
    let mut fake = Fake::default()
        .answer_times("systemctl --user is-active", "", 1)
        .fail("systemctl --user is-active", "")
        .fail("pgrep", "");
    let mut holders = FakeHolders::default();
    let (result, said) = run(
        &setup,
        quick(false),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    result.unwrap();

    assert_eq!(
        said,
        [
            "Stopped the maxd service.".to_owned(),
            format!("Removed {}.", setup.file().display()),
        ]
    );
    assert_eq!(
        without_pgrep(&fake),
        [
            "systemctl --user is-active --quiet maxd.service",
            "systemctl --user disable --now maxd.service",
            "systemctl --user is-active --quiet maxd.service",
            "systemctl --user daemon-reload",
        ]
    );
    assert!(!setup.file().exists());
    assert!(
        !fake.ran.iter().any(|line| line.contains("maxd-vm")),
        "VM helpers run on macOS only"
    );

    let mut fake = Fake::default()
        .fail("systemctl --user is-active", "")
        .fail("pgrep", "");
    let (result, said) = run(
        &setup,
        quick(false),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    result.unwrap();
    assert_eq!(said, [NOTHING]);
    assert_eq!(
        without_pgrep(&fake),
        ["systemctl --user is-active --quiet maxd.service"]
    );
}

#[test]
fn a_service_that_wont_stop_fails_before_anything_else_goes() {
    let dir = TempDir::new();
    let setup = setup(&dir, Manager::Launchd);
    install(&setup, &mut Fake::default()).unwrap();
    let mut fake = Fake::default()
        .answer("launchctl print", "")
        .fail("pgrep", "");
    let mut holders = FakeHolders::default();
    let (result, said) = run(
        &setup,
        quick(true),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    let err = result.unwrap_err();
    assert!(err.to_string().contains("still loaded"), "{err}");
    assert!(said.is_empty(), "{said:?}");
    assert_eq!(holders.calls, 0);
    assert!(setup.file().exists());

    let setup = setup_systemd(&dir);
    let mut fake = Fake::default().fail("pgrep", "");
    let (result, _) = run(
        &setup,
        quick(false),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    let err = result.unwrap_err();
    assert!(err.to_string().contains("still running"), "{err}");
    assert_eq!(holders.calls, 0);
}

fn setup_systemd(dir: &TempDir) -> Setup {
    let setup = setup(dir, Manager::Systemd);
    install(&setup, &mut Fake::default()).unwrap();
    setup
}

/// Fills a home folder the way maxd and the app leave it.
fn fill(setup: &Setup) {
    let data = setup.data.as_ref().unwrap();
    write(&data.join("maxd.db"), "sessions");
    write(&data.join("hold/one/holder.json"), "{}");
    write(&data.join(APP_SSH).join("config"), "the app's");
    write(&setup.runtime.as_ref().unwrap().join("hold/one.sock"), "");
    write(
        &setup.home.join("Library/Logs/MaxHQ/maxd.log"),
        "what maxd logged",
    );
    write(&setup.home.join(".config/maxhq/ssh/id_ed25519"), "key");
    write(&setup.home.join(".config/maxhq/maxhq-host.toml"), "yours");
    write(
        &setup
            .home
            .join("Library/Application Support/com.maxagenthq.maxhq/settings.json"),
        "the app's",
    );
}

#[test]
fn delete_data_deletes_maxds_data_but_not_the_apps() {
    let dir = TempDir::new();
    let setup = setup(&dir, Manager::Launchd);
    install(&setup, &mut Fake::default()).unwrap();
    fill(&setup);
    let data = setup.data.clone().unwrap();
    // A link in the data folder is removed, not followed.
    let elsewhere = dir.path().join("elsewhere");
    write(&elsewhere.join("keep"), "yours");
    std::os::unix::fs::symlink(&elsewhere, data.join("link")).unwrap();

    let mut fake = Fake::default().fail("pgrep", "");
    let mut holders = FakeHolders::default();
    let (result, said) = run(
        &setup,
        quick(true),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    result.unwrap();

    let home = &setup.home;
    assert_eq!(
        said,
        [
            format!("Removed {}.", setup.file().display()),
            format!(
                "Deleted maxd's data in {}, but kept {}, which the MaxHQ app uses.",
                data.display(),
                data.join("ssh").display()
            ),
            format!("Deleted {}.", setup.runtime.as_ref().unwrap().display()),
            format!("Deleted {}.", home.join("Library/Logs/MaxHQ").display()),
            format!("Deleted {}.", home.join(".config/maxhq/ssh").display()),
        ]
    );
    let names: Vec<_> = fs::read_dir(&data)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["ssh"]);
    assert_eq!(
        fs::read_to_string(data.join("ssh/config")).unwrap(),
        "the app's"
    );
    assert!(!setup.runtime.as_ref().unwrap().exists());
    assert!(!home.join("Library/Logs/MaxHQ").exists());
    assert!(!home.join(".config/maxhq/ssh").exists());
    assert!(home.join(".config/maxhq/maxhq-host.toml").exists());
    assert!(home
        .join("Library/Application Support/com.maxagenthq.maxhq/settings.json")
        .exists());
    assert!(elsewhere.join("keep").exists());

    // Again, there's nothing left to delete.
    let mut fake = Fake::default().fail("pgrep", "");
    let (result, said) = run(
        &setup,
        quick(true),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    result.unwrap();
    assert_eq!(said, [NOTHING]);
}

#[test]
fn delete_data_deletes_the_whole_folder_when_the_app_keeps_nothing_there() {
    let dir = TempDir::new();
    let setup = setup_systemd(&dir);
    let data = setup.data.clone().unwrap();
    write(&data.join("maxd.db"), "sessions");
    let mut fake = Fake::default()
        .fail("systemctl --user is-active", "")
        .fail("pgrep", "");
    let mut holders = FakeHolders::default();
    let (result, said) = run(
        &setup,
        quick(true),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    result.unwrap();
    assert!(
        said.contains(&format!("Deleted {}.", data.display())),
        "{said:?}"
    );
    assert!(!data.exists());
    assert!(
        !said.iter().any(|line| line.contains("Logs")),
        "systemd keeps the log in the journal"
    );
}

#[test]
fn agents_that_may_still_run_keep_the_data() {
    let dir = TempDir::new();
    let setup = setup(&dir, Manager::Launchd);
    install(&setup, &mut Fake::default()).unwrap();
    fill(&setup);
    let mut fake = Fake::default().fail("pgrep", "");
    let left = "agent one may still run as process 7: it didn't prove it's the session's own";
    let mut holders = FakeHolders::ending(1, &[left]);
    let (result, said) = run(
        &setup,
        quick(true),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    assert_eq!(
        problems(result),
        [left, "kept maxd's data, since an agent may still use it"]
    );
    assert_eq!(
        said,
        [
            "Ended 1 running agent.".to_owned(),
            format!("Removed {}.", setup.file().display()),
        ]
    );
    assert!(setup.data.as_ref().unwrap().join("maxd.db").exists());
    assert!(setup.home.join("Library/Logs/MaxHQ/maxd.log").exists());
}

#[test]
fn where_the_nixos_module_owns_the_unit_uninstall_acts_as_before() {
    let dir = TempDir::new();
    let setup = setup(&dir, Manager::Systemd);
    module_unit(&dir);
    write(&setup.file(), "an older install");
    fill(&setup);
    let mut holders = FakeHolders::default();

    let mut fake = Fake::default();
    let (result, said) = run(
        &setup,
        quick(true),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    assert!(
        matches!(result, Err(ServiceError::ModuleKeepsData)),
        "{result:?}"
    );
    assert!(
        said.is_empty() && fake.ran.is_empty(),
        "{said:?} {:?}",
        fake.ran
    );
    assert!(setup.file().exists());
    assert!(setup.data.as_ref().unwrap().join("maxd.db").exists());

    let mut fake = Fake::default();
    let (result, said) = run(
        &setup,
        quick(false),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    result.unwrap();
    assert_eq!(
        fake.ran,
        [
            "systemctl --user disable --now maxd.service",
            "systemctl --user daemon-reload",
        ]
    );
    assert_eq!(
        said,
        [format!(
            "Stopped maxd and removed {}.",
            setup.file().display()
        )]
    );
    assert!(!setup.file().exists());
    assert_eq!(holders.calls, 0);
    assert!(dir.path().join("etc/systemd/user/maxd.service").exists());
}

#[test]
fn maxd_uninstalling_itself_stops_its_service_last() {
    let dir = TempDir::new();
    let setup = setup(&dir, Manager::Launchd);
    install(&setup, &mut Fake::default()).unwrap();
    fill(&setup);
    let mut fake = Fake::default()
        .answer("launchctl print", "")
        .fail("pgrep", "");
    let mut holders = FakeHolders::ending(1, &[]);
    let how = Uninstall {
        itself: true,
        ..quick(false)
    };
    let (result, said) = run(&setup, how, &Clis::default(), &mut fake, &mut holders);
    result.unwrap();
    assert_eq!(
        said,
        [
            "Ended 1 running agent.".to_owned(),
            format!("Removed {}.", setup.file().display()),
            "Stopping the maxd service.".to_owned(),
        ]
    );
    assert_eq!(
        without_pgrep(&fake),
        [format!("launchctl bootout {TARGET}")]
    );
    assert!(
        !fake.ran.iter().any(|line| line.contains("maxd-vm")),
        "its own VM helpers end with it"
    );
    assert!(setup.data.as_ref().unwrap().join("maxd.db").exists());

    // On Linux it's disabled while the unit file is there, and stopped last.
    let setup = setup_systemd(&dir);
    let mut fake = Fake::default().fail("pgrep", "");
    let (result, _) = run(
        &setup,
        how,
        &Clis::default(),
        &mut fake,
        &mut FakeHolders::default(),
    );
    result.unwrap();
    assert_eq!(
        without_pgrep(&fake),
        [
            "systemctl --user disable maxd.service",
            "systemctl --user daemon-reload",
            "systemctl --user stop maxd.service",
        ]
    );
    assert!(!setup.file().exists());
}

#[test]
fn the_update_check_stops_before_the_service() {
    let dir = TempDir::new();
    let setup = setup(&dir, Manager::Launchd);
    install(&setup, &mut Fake::default()).unwrap();
    let watch = "pgrep -U 501 -f -- maxd update watch";
    let mut fake = Fake::default().answer_times(watch, "", 1).fail("pgrep", "");
    let mut holders = FakeHolders::default();
    let (result, said) = run(
        &setup,
        quick(false),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    result.unwrap();
    assert_eq!(said[0], "Stopped the check on a recent update.");
    assert_eq!(
        fake.ran[..3],
        [
            "pgrep -U 501 -f -- maxd update watch --build ",
            "pkill -U 501 -f -- maxd update watch --build ",
            "pgrep -U 501 -f -- maxd update watch --build ",
        ]
    );
}

#[test]
fn vm_helpers_get_a_moment_to_end_on_their_own() {
    let dir = TempDir::new();
    let setup = setup(&dir, Manager::Launchd);
    let vm = "pgrep -U 501 -P 1 -x -- maxd-vm";
    // Running at first, and for one more look.
    let mut fake = Fake::default().answer_times(vm, "", 2).fail("pgrep", "");
    let mut holders = FakeHolders::default();
    let (result, said) = run(
        &setup,
        quick(false),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    result.unwrap();
    assert_eq!(said, ["Stopped maxd's VM helpers."]);
    assert_eq!(fake.ran.iter().filter(|line| *line == vm).count(), 3);
    assert!(!fake.ran.iter().any(|line| line.starts_with("pkill")));
}

#[test]
fn agent_device_is_found_by_where_maxd_installed_it() {
    let dir = TempDir::new();
    let mut setup = setup(&dir, Manager::Launchd);
    setup.data = Some(PathBuf::from("/data/max.d+1"));
    let pattern = r"/data/max\.d\+1/tools/agent-device/";
    let mut fake = Fake::default()
        .answer_times("pgrep -U 501 -f -- /data", "", 1)
        .fail("pgrep", "");
    let mut holders = FakeHolders::default();
    let (result, said) = run(
        &setup,
        quick(false),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    result.unwrap();
    assert_eq!(
        said,
        ["Stopped agent-device, which drove your test devices."]
    );
    let at = fake
        .ran
        .iter()
        .position(|line| line.ends_with(pattern))
        .unwrap();
    assert_eq!(
        fake.ran[at..at + 3],
        [
            format!("pgrep -U 501 -f -- {pattern}"),
            format!("pkill -U 501 -f -- {pattern}"),
            format!("pgrep -U 501 -f -- {pattern}"),
        ]
    );
}

#[test]
fn an_emulator_that_wont_stop_is_killed_then_reported() {
    let dir = TempDir::new();
    let setup = setup(&dir, Manager::Launchd);
    let emulator = "pgrep -U 501 -f -- -avd mhq-test( |$)";
    let mut fake = Fake::default().answer(emulator, "").fail("pgrep", "");
    let mut holders = FakeHolders::default();
    let (result, said) = run(
        &setup,
        quick(false),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    assert_eq!(
        problems(result),
        ["the Android test emulator is still running"]
    );
    assert_eq!(said, [NOTHING]);
    let kills: Vec<_> = fake
        .ran
        .iter()
        .filter(|line| line.starts_with("pkill"))
        .collect();
    assert_eq!(
        kills,
        [
            "pkill -U 501 -f -- -avd mhq-test( |$)",
            "pkill -KILL -U 501 -f -- -avd mhq-test( |$)",
        ]
    );
    let looks = fake.ran.iter().filter(|line| *line == emulator).count();
    assert_eq!(looks, 1 + 2 * STOP_TRIES as usize);
}

#[test]
fn the_test_simulator_is_shut_down_and_deleted_with_the_data() {
    let dir = TempDir::new();
    let setup = setup(&dir, Manager::Launchd);
    let data = setup.data.clone().unwrap();
    let udid = "5D8E3B2A-0000-4000-8000-000000000001";
    write(&mac_host::udid_file(&data), &format!("{udid}\n"));

    let mut fake = Fake::default().fail("pgrep", "");
    let mut holders = FakeHolders::default();
    let (result, said) = run(
        &setup,
        quick(false),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    result.unwrap();
    assert_eq!(said, ["Shut down the iOS test Simulator."]);
    assert_eq!(changes(&fake), [format!("xcrun simctl shutdown {udid}")]);

    // Already shut down, so there's nothing to say, and with the data it
    // goes.
    let mut fake = Fake::default()
        .fail(
            "xcrun simctl shutdown",
            "Unable to shutdown device in current state: Shutdown",
        )
        .fail("pgrep", "");
    let (result, said) = run(
        &setup,
        quick(true),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    result.unwrap();
    assert_eq!(said[0], "Deleted the iOS test Simulator.");
    assert_eq!(
        changes(&fake),
        [
            format!("xcrun simctl shutdown {udid}"),
            format!("xcrun simctl delete {udid}"),
        ]
    );
    assert!(!data.exists());
}

#[test]
fn the_ssh_connections_maxd_kept_open_are_closed() {
    let dir = TempDir::new();
    let setup = setup(&dir, Manager::Launchd);
    let runtime = setup.runtime.clone().unwrap();
    write(&runtime.join(".ssh-connectionAbC123/master"), "");
    fs::create_dir_all(runtime.join(".ssh-connectionDeF456")).unwrap();
    write(&runtime.join("other/master"), "");
    let mut fake = Fake::default().fail("pgrep", "");
    let mut holders = FakeHolders::default();
    let (result, said) = run(
        &setup,
        quick(false),
        &Clis::default(),
        &mut fake,
        &mut holders,
    );
    result.unwrap();
    assert_eq!(said, ["Closed 1 SSH connection to your other devices."]);
    assert_eq!(
        changes(&fake),
        [format!(
            "ssh -S {} -O exit maxhq",
            runtime.join(".ssh-connectionAbC123/master").display()
        )]
    );
}

#[test]
fn only_maxds_funnel_paths_are_closed() {
    let dir = TempDir::new();
    let setup = setup(&dir, Manager::Launchd);
    let status = r#"{
        "TCP": {"443": {"HTTPS": true}},
        "Web": {
            "devbox.example.ts.net:443": {"Handlers": {
                "/maxhq/t2": {"Proxy": "http://127.0.0.1:41234"},
                "/maxhq/t1": {"Proxy": "http://127.0.0.1:41234"},
                "/yours": {"Proxy": "http://127.0.0.1:3000"}
            }},
            "devbox.example.ts.net:8443": {"Handlers": {
                "/maxhq/t9": {"Proxy": "http://127.0.0.1:41234"}
            }}
        }
    }"#;
    let cli = "/usr/bin/tailscale";
    let mut fake = Fake::default()
        .answer("/usr/bin/tailscale serve status", status)
        .fail(
            "/usr/bin/tailscale funnel --https=443 --set-path=/maxhq/t2",
            "no",
        )
        .fail("pgrep", "");
    let clis = Clis {
        tailscale: Some(cli.into()),
        incus: None,
    };
    let mut holders = FakeHolders::default();
    let (result, said) = run(&setup, quick(false), &clis, &mut fake, &mut holders);
    assert_eq!(
        problems(result),
        ["couldn't close the Tailscale Funnel path /maxhq/t2: no"]
    );
    assert_eq!(said, ["Closed 1 Tailscale Funnel path for webhooks."]);
    assert_eq!(
        changes(&fake),
        [
            "/usr/bin/tailscale serve status --json",
            "/usr/bin/tailscale funnel --https=443 --set-path=/maxhq/t1 off",
            "/usr/bin/tailscale funnel --https=443 --set-path=/maxhq/t2 off",
        ]
    );
    assert!(funnel_paths("not json").is_empty());
    assert!(funnel_paths("{}").is_empty());
}

#[test]
fn the_linux_test_container_stops_and_workspaces_stay() {
    let dir = TempDir::new();
    let setup = setup(&dir, Manager::Systemd);
    let listed = "mhq-test-linux,RUNNING\nmhq-ws-one,RUNNING\nmhq-ws-two,STOPPED\n";
    let clis = Clis {
        tailscale: None,
        incus: Some("/usr/bin/incus".into()),
    };
    let mut holders = FakeHolders::default();
    for (delete_data, verb, done) in [(false, "stop", "Stopped"), (true, "delete", "Deleted")] {
        let mut fake = Fake::default()
            .answer("/usr/bin/incus list", listed)
            .fail("systemctl --user is-active", "")
            .fail("pgrep", "");
        let (result, said) = run(&setup, quick(delete_data), &clis, &mut fake, &mut holders);
        result.unwrap();
        assert_eq!(
            said,
            [
                format!("{done} the Linux test container."),
                "Left 2 workspace containers in Incus, since they hold your work: mhq-ws-one, mhq-ws-two."
                    .to_owned(),
            ]
        );
        assert_eq!(
            changes(&fake),
            [
                "/usr/bin/incus list user.maxhq.managed=true --format csv --columns ns".to_owned(),
                format!("/usr/bin/incus {verb} --force mhq-test-linux"),
            ]
        );
    }

    // A stopped test container is left as it is.
    let mut fake = Fake::default()
        .answer("/usr/bin/incus list", "mhq-test-linux,STOPPED\n")
        .fail("systemctl --user is-active", "")
        .fail("pgrep", "");
    let (result, said) = run(&setup, quick(false), &clis, &mut fake, &mut holders);
    result.unwrap();
    assert_eq!(said, [NOTHING]);
}

#[test]
fn a_pattern_matches_only_itself() {
    assert_eq!(
        literal(r"/a b/c.d+e(f)[g]{h}^$|?*\"),
        r"/a b/c\.d\+e\(f\)\[g\]\{h\}\^\$\|\?\*\\"
    );
    assert_eq!(count(1, "agent"), "1 agent");
    assert_eq!(count(3, "agent"), "3 agents");
}
