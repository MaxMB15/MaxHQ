// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Holding the system out of idle sleep, or the display on as well, and
//! reading whether the machine runs on external power. `maxd` holds the
//! system while its agents work, and the app holds the display while its
//! sun is on.
//!
//! Each system lists a hold with its reason: Activity Monitor and `pmset
//! -g assertions` on macOS, `systemd-inhibit --list` on Linux, `powercfg
//! /requests` on Windows. Each also lets a hold go when the process that
//! took it ends, so a crash can't leave one held. Closing the lid still
//! sleeps the machine under any of them.

use std::io;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as imp;
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod other;
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
use other as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

// Linux reads the charger from the kernel's list of power supplies. The
// tests read fake lists on any system.
#[cfg(any(target_os = "linux", test))]
mod supplies;

/// What a hold keeps from happening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    /// Sleep after the machine sits idle. The display still dims and
    /// turns off.
    System,
    /// The display dimming and turning off after the machine sits idle,
    /// and the machine sleeping.
    Display,
}

impl Keep {
    /// Whether this system can take this hold.
    pub const fn supported(self) -> bool {
        imp::supports(self)
    }
}

/// A hold. Dropping it lets go.
pub struct Awake(#[allow(dead_code)] imp::Hold);

impl Awake {
    /// Takes a hold against `keep`, which the system lists with `reason`.
    pub fn take(keep: Keep, reason: &str) -> io::Result<Self> {
        imp::take(keep, reason).map(Self)
    }
}

/// Whether the system runs on external power, or has no battery.
pub fn on_charger() -> io::Result<bool> {
    imp::on_charger()
}

/// What the power status's AC line and battery flags say about the
/// charger: 1 is on external power, and the 128 bit is no battery.
/// Unknown counts as on battery.
#[cfg_attr(not(windows), allow(dead_code))]
fn charging(ac_line: u8, battery: u8) -> bool {
    /// `ACLineStatus` on external power.
    const ONLINE: u8 = 1;
    /// `BatteryFlag`'s bit for a system with no battery.
    const NO_BATTERY: u8 = 128;
    ac_line == ONLINE || (battery != u8::MAX && battery & NO_BATTERY != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_charger_is_external_power_or_no_battery() {
        // On the charger, with a battery charging or full.
        assert!(charging(1, 8));
        assert!(charging(1, 1));
        // A desktop: no battery, and the line said or not.
        assert!(charging(1, 128));
        assert!(charging(255, 128));
        // On battery, and unknown with a battery or not.
        assert!(!charging(0, 1));
        assert!(!charging(0, 4));
        assert!(!charging(255, 255));
        assert!(!charging(255, 2));
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn each_hold_is_taken_and_let_go() {
        for keep in [Keep::System, Keep::Display] {
            assert!(keep.supported());
            let first = Awake::take(keep, "MaxHQ's tests").unwrap();
            let second = Awake::take(keep, "MaxHQ's tests").unwrap();
            drop(first);
            drop(second);
        }
        // It reads either way, on a machine with a battery or none.
        on_charger().unwrap();
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    #[test]
    fn other_systems_hold_nothing() {
        for keep in [Keep::System, Keep::Display] {
            assert!(!keep.supported());
            let err = Awake::take(keep, "a test").err().unwrap();
            assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        }
    }
}
