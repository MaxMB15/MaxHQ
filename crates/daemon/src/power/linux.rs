// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Linux's hold: a logind inhibitor that a `systemd-inhibit` child keeps
//! for as long as the hold lasts, and the kernel's list of power supplies
//! for the charger.
//!
//! The child runs `cat` on a pipe from `maxd`, so if `maxd` dies without
//! letting go, `cat` reads the end of its input, exits, and the inhibitor
//! goes with it. logind's default `LidSwitchIgnoreInhibited=yes` means
//! closing the lid still sleeps the machine, as on a Mac.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tracing::{debug, warn};

use super::awake::Holder;

/// What `systemd-inhibit --list` shows as the reason.
const REASON: &str = "MaxHQ is keeping this computer awake";

/// Where the kernel lists the power supplies.
const SUPPLIES: &str = "/sys/class/power_supply";

/// Whether a failed hold was logged as a warning already. Later ones only
/// show in the debug log, since they'd fail the same way each time.
static WARNED: AtomicBool = AtomicBool::new(false);

/// Linux's power management, through logind.
pub struct System {
    /// The program that holds the inhibitor, and its arguments.
    program: OsString,
    args: Vec<OsString>,
    /// The folder the power supplies are listed in.
    supplies: PathBuf,
}

/// The holder `maxd` uses: logind through `systemd-inhibit`, and the
/// kernel's supply list.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn holder() -> System {
    let why = format!("--why={REASON}");
    System::new(
        "systemd-inhibit",
        [
            "--what=idle:sleep",
            "--who=MaxHQ",
            &why,
            "--mode=block",
            "cat",
        ],
        SUPPLIES,
    )
}

impl System {
    /// One that holds by running `program` with `args`, and reads the
    /// supplies in `supplies`.
    fn new<I, S>(program: impl Into<OsString>, args: I, supplies: impl Into<PathBuf>) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            supplies: supplies.into(),
        }
    }
}

impl Holder for System {
    type Hold = Inhibitor;
    const MACHINE: &'static str = "this computer";
    const CHECK_EVERY: Duration = Duration::from_secs(15);

    /// Starts the child that holds the inhibitor, or none when it can't
    /// start. A child that starts and then fails, such as on a machine
    /// without logind, is logged when it ends.
    fn take(&self) -> Option<Inhibitor> {
        let started = Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();
        match started {
            Ok(child) => {
                debug!("keeping this computer awake");
                Some(Inhibitor::watch(child))
            }
            Err(error) => {
                refused(&format!(
                    "couldn't run {}: {error}",
                    self.program.to_string_lossy()
                ));
                None
            }
        }
    }

    fn on_charger(&self) -> bool {
        on_charger_in(&self.supplies)
    }
}

/// The running `systemd-inhibit`, which holds the inhibitor until it's
/// dropped.
pub struct Inhibitor {
    /// Shared with the thread that reads its errors, which reaps it if it
    /// fails by itself.
    child: Arc<Mutex<Child>>,
    /// Its input. Closing it ends `cat`, and so the inhibitor.
    stdin: Option<ChildStdin>,
}

impl Inhibitor {
    /// Keeps `child`, with a thread that logs what it writes to stderr if
    /// it fails.
    fn watch(mut child: Child) -> Self {
        let stdin = child.stdin.take();
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(child));
        let shared = child.clone();
        let reader = std::thread::Builder::new()
            .name("systemd-inhibit errors".into())
            .spawn(move || {
                let mut written = String::new();
                if let Some(mut stderr) = stderr {
                    // It only ends once the child and its `cat` are gone.
                    let _ = stderr.read_to_string(&mut written);
                }
                let status = shared
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .try_wait();
                if let Ok(Some(status)) = status
                    && !status.success()
                    && !written.trim().is_empty()
                {
                    refused(&format!("systemd-inhibit failed: {}", written.trim()));
                }
            });
        if let Err(error) = reader {
            debug!(%error, "couldn't watch systemd-inhibit for errors");
        }
        Self { child, stdin }
    }

    /// The child's process ID.
    #[cfg(all(test, target_os = "linux"))]
    fn id(&self) -> u32 {
        self.child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .id()
    }
}

impl Drop for Inhibitor {
    fn drop(&mut self) {
        // Closing the input first ends `cat` even if the kill fails.
        drop(self.stdin.take());
        let mut child = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        // Killing `systemd-inhibit` closes the inhibitor at once. It may
        // have ended already, which is fine.
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Logs that the system wouldn't hold, as a warning the first time.
fn refused(reason: &str) {
    if WARNED.swap(true, Ordering::Relaxed) {
        debug!(%reason, "Linux refused to keep this computer awake");
    } else {
        warn!(%reason, "Linux refused to keep this computer awake");
    }
}

/// Whether the supplies listed in `dir` say the machine runs on external
/// power. Any supply other than a battery or a UPS that's online counts.
/// A machine with no battery of its own, such as a desktop, counts as on
/// the charger, even when it lists a wireless mouse's battery.
fn on_charger_in(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return true;
    };
    let mut battery = false;
    for entry in entries.flatten() {
        let supply = entry.path();
        match read(&supply, "type").as_deref() {
            Some("Battery") => battery |= read(&supply, "scope").as_deref() != Some("Device"),
            Some("UPS") | None => {}
            Some(_) => {
                if read(&supply, "online").as_deref() == Some("1") {
                    return true;
                }
            }
        }
    }
    !battery
}

/// A supply's attribute, trimmed, or none when it can't be read.
fn read(supply: &Path, attribute: &str) -> Option<String> {
    let value = std::fs::read_to_string(supply.join(attribute)).ok()?;
    Some(value.trim().to_owned())
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    use std::time::Instant;

    #[cfg(target_os = "linux")]
    use maxhq_protocol::model::KeepAwake;

    #[cfg(target_os = "linux")]
    use super::super::awake::Awake;
    #[cfg(target_os = "linux")]
    use super::super::Power;
    use super::*;

    /// A folder of fake power supplies, removed when dropped.
    struct Supplies(PathBuf);

    impl Supplies {
        fn new(supplies: &[(&str, &[(&str, &str)])]) -> Self {
            let dir = std::env::temp_dir().join(format!("maxhq-supplies-{}", uuid::Uuid::now_v7()));
            for (name, attributes) in supplies {
                let supply = dir.join(name);
                std::fs::create_dir_all(&supply).unwrap();
                for (attribute, value) in *attributes {
                    std::fs::write(supply.join(attribute), format!("{value}\n")).unwrap();
                }
            }
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Supplies {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const BATTERY: (&str, &[(&str, &str)]) =
        ("BAT0", &[("type", "Battery"), ("status", "Discharging")]);

    #[test]
    fn a_laptop_is_on_the_charger_only_while_plugged_in() {
        let unplugged = Supplies::new(&[BATTERY, ("AC", &[("type", "Mains"), ("online", "0")])]);
        assert!(!on_charger_in(&unplugged.0));
        let plugged = Supplies::new(&[BATTERY, ("AC", &[("type", "Mains"), ("online", "1")])]);
        assert!(on_charger_in(&plugged.0));
        let usb_c = Supplies::new(&[
            BATTERY,
            ("AC", &[("type", "Mains"), ("online", "0")]),
            (
                "ucsi-source-psy-USBC000:001",
                &[("type", "USB"), ("online", "1")],
            ),
        ]);
        assert!(on_charger_in(&usb_c.0));
        let no_adapter_listed = Supplies::new(&[BATTERY]);
        assert!(!on_charger_in(&no_adapter_listed.0));
    }

    #[test]
    fn a_machine_without_a_battery_of_its_own_is_on_the_charger() {
        let desktop = Supplies::new(&[]);
        assert!(on_charger_in(&desktop.0));
        let mouse = Supplies::new(&[(
            "hidpp_battery_0",
            &[("type", "Battery"), ("scope", "Device")],
        )]);
        assert!(on_charger_in(&mouse.0));
        let ups = Supplies::new(&[("ups", &[("type", "UPS"), ("online", "0")])]);
        assert!(on_charger_in(&ups.0));
        assert!(on_charger_in(&desktop.0.join("missing")));
    }

    #[test]
    fn a_supply_without_its_online_file_is_offline() {
        let supplies = Supplies::new(&[BATTERY, ("AC", &[("type", "Mains")])]);
        assert!(!on_charger_in(&supplies.0));
    }

    /// Whether a process with `pid` is still around, zombies included.
    #[cfg(target_os = "linux")]
    fn exists(pid: u32) -> bool {
        Path::new(&format!("/proc/{pid}")).exists()
    }

    /// Waits up to five seconds for `done`.
    #[cfg(target_os = "linux")]
    fn eventually(mut done: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        done()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_child_is_started_once_and_reaped_when_let_go() {
        let supplies = Supplies::new(&[]);
        let awake = Awake::new(System::new("cat", Vec::<OsString>::new(), &supplies.0));
        let pid =
            |awake: &Awake<System>| awake.state.lock().unwrap().held.as_ref().map(Inhibitor::id);
        awake.set_mode(KeepAwake::Working);
        assert!(pid(&awake).is_none());
        awake.set_busy(true);
        let first = pid(&awake).unwrap();
        assert!(exists(first));
        awake.set_busy(true);
        awake.set_mode(KeepAwake::Always);
        assert_eq!(pid(&awake), Some(first));
        awake.set_mode(KeepAwake::Off);
        assert!(pid(&awake).is_none());
        // Dropping it waited for it, so not even a zombie is left.
        assert!(!exists(first));
    }

    #[test]
    fn a_missing_program_holds_nothing() {
        let system = System::new(
            "/nonexistent/systemd-inhibit",
            Vec::<OsString>::new(),
            SUPPLIES,
        );
        assert!(system.take().is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_child_that_fails_by_itself_is_reaped() {
        let system = System::new(
            "sh",
            ["-c", "echo 'Failed to inhibit: Access denied' >&2; exit 1"],
            SUPPLIES,
        );
        let held = system.take().unwrap();
        let pid = held.id();
        assert!(eventually(|| !exists(pid)), "{pid} wasn't reaped");
        drop(held);
    }
}
