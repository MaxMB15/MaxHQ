// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Holding Windows out of idle sleep with a power request, which
//! `powercfg /requests` lists with its reason, and reading whether the PC
//! runs on external power. The display still turns off, and closing the
//! lid still sleeps it.
//!
//! A power request holds whichever thread lets it go, unlike
//! `SetThreadExecutionState`, which holds only while the thread that
//! called it lives. macOS's hold is in `maxhq_core::power`, and Linux's is
//! a logind inhibitor in `maxd`, so on Unix these fail as unsupported.

use std::io;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

/// Keeps the system out of idle sleep until it's dropped.
#[derive(Debug)]
pub struct Awake(#[allow(dead_code)] imp::Request);

impl Awake {
    /// Takes a hold, which the system lists with `reason`.
    pub fn take(reason: &str) -> io::Result<Self> {
        imp::take(reason).map(Self)
    }
}

/// Whether the system runs on external power, or has no battery.
pub fn on_charger() -> io::Result<bool> {
    imp::on_charger()
}

/// What the power status's AC line and battery flags say about the
/// charger: 1 is on external power, and the 128 bit is no battery.
/// Unknown counts as on battery.
#[cfg_attr(unix, allow(dead_code))]
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

    #[cfg(unix)]
    #[test]
    fn unix_has_its_own_holds() {
        let err = Awake::take("a test").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }

    #[cfg(windows)]
    #[test]
    fn windows_holds_and_lets_go() {
        let first = Awake::take("MaxHQ's tests are keeping this PC awake").unwrap();
        let second = Awake::take("MaxHQ's tests are keeping this PC awake").unwrap();
        drop(first);
        drop(second);
        // It reads either way, on a runner with a battery or none.
        on_charger().unwrap();
    }
}
