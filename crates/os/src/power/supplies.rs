// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The kernel's list of power supplies, which says whether a Linux
//! machine runs on the charger.

use std::path::Path;

/// Where the kernel lists the power supplies.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const SUPPLIES: &str = "/sys/class/power_supply";

/// Whether the supplies listed in `dir` say the machine runs on external
/// power. Any supply other than a battery or a UPS that's online counts.
/// A machine with no battery of its own, such as a desktop, counts as on
/// the charger, even when it lists a wireless mouse's battery.
pub fn on_charger_in(dir: &Path) -> bool {
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
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A folder of fake power supplies, removed when dropped.
    struct Supplies(PathBuf);

    impl Supplies {
        fn new(supplies: &[(&str, &[(&str, &str)])]) -> Self {
            static MADE: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "maxhq-os-supplies-{}-{}",
                std::process::id(),
                MADE.fetch_add(1, Ordering::Relaxed)
            ));
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
        // Linux names a USB-C port with a `:`, which Windows can't put in a
        // file's name.
        let port = if cfg!(windows) {
            "ucsi-source-psy-USBC000-001"
        } else {
            "ucsi-source-psy-USBC000:001"
        };
        let usb_c = Supplies::new(&[
            BATTERY,
            ("AC", &[("type", "Mains"), ("online", "0")]),
            (port, &[("type", "USB"), ("online", "1")]),
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
}
