// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The Mac's hold: an IOKit assertion that stops idle sleep, and IOKit's
//! battery estimate for the charger.

use std::ffi::CStr;
use std::time::Duration;

use maxhq_core::power::{Assertion, Prevent};
use tracing::{debug, warn};

use super::awake::Holder;

/// What Activity Monitor and `pmset -g assertions` show.
const REASON: &CStr = c"MaxHQ is keeping this Mac awake";
/// `kIOPSTimeRemainingUnlimited`, the estimate on external power and on a
/// Mac with no battery.
const UNLIMITED: f64 = -2.0;

#[allow(unsafe_code)]
#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPSGetTimeRemainingEstimate() -> f64;
}

/// The Mac's power management.
pub struct System;

/// The holder `maxd` uses.
pub fn holder() -> System {
    System
}

impl Holder for System {
    type Hold = Assertion;
    const MACHINE: &'static str = "the Mac";
    const CHECK_EVERY: Duration = Duration::from_secs(15);

    /// Takes the assertion, or none when macOS refuses.
    fn take(&self) -> Option<Assertion> {
        match Assertion::take(Prevent::IdleSleep, REASON) {
            Ok(held) => {
                debug!("keeping the Mac awake");
                Some(held)
            }
            Err(error) => {
                warn!(%error, "macOS refused to keep the Mac awake");
                None
            }
        }
    }

    fn on_charger(&self) -> bool {
        is_unlimited(time_remaining())
    }
}

/// IOKit's estimate of the time left on battery.
#[allow(unsafe_code)]
fn time_remaining() -> f64 {
    // SAFETY: it takes nothing and only reads.
    unsafe { IOPSGetTimeRemainingEstimate() }
}

/// Whether the estimate means there's no battery to run down.
fn is_unlimited(estimate: f64) -> bool {
    estimate == UNLIMITED
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::model::KeepAwake;

    use super::super::awake::Awake;
    use super::super::Power;
    use super::*;

    fn held(awake: &Awake<System>) -> Option<u32> {
        awake.state.lock().unwrap().held.as_ref().map(Assertion::id)
    }

    #[test]
    fn the_assertion_is_taken_once_and_let_go() {
        let awake = Awake::new(System);
        awake.set_mode(KeepAwake::Working);
        assert!(held(&awake).is_none());
        awake.set_busy(true);
        let first = held(&awake);
        assert!(first.is_some());
        awake.set_busy(true);
        assert_eq!(held(&awake), first);
        awake.set_busy(false);
        assert!(held(&awake).is_none());
    }

    #[test]
    fn only_an_unlimited_estimate_reads_as_the_charger() {
        assert!(is_unlimited(UNLIMITED));
        // Still working it out, and then the time left.
        assert!(!is_unlimited(-1.0));
        assert!(!is_unlimited(0.0));
        assert!(!is_unlimited(7200.0));
        let now = time_remaining();
        assert!(now == UNLIMITED || now == -1.0 || now >= 0.0, "{now}");
    }
}
