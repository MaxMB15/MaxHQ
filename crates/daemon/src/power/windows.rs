// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Windows's hold: a power request that keeps the PC out of idle sleep,
//! which `powercfg /requests` lists, and the power status for the charger.

use std::time::Duration;

use maxhq_os::power::{self, Awake};
use tracing::{debug, warn};

use super::awake::Holder;

/// What `powercfg /requests` shows.
const REASON: &str = "MaxHQ is keeping this PC awake";

/// Windows's power management.
pub struct System;

/// The holder `maxd` uses.
pub fn holder() -> System {
    System
}

impl Holder for System {
    type Hold = Awake;
    const MACHINE: &'static str = "the PC";
    const CHECK_EVERY: Duration = Duration::from_secs(15);

    /// Takes a power request, or none when Windows refuses.
    fn take(&self) -> Option<Awake> {
        match Awake::take(REASON) {
            Ok(held) => {
                debug!("keeping the PC awake");
                Some(held)
            }
            Err(error) => {
                warn!(%error, "Windows refused to keep the PC awake");
                None
            }
        }
    }

    /// A status Windows won't give reads as on battery, so the charger
    /// mode holds only while an agent works.
    fn on_charger(&self) -> bool {
        power::on_charger().unwrap_or_else(|error| {
            debug!(%error, "couldn't read the PC's power status");
            false
        })
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::model::KeepAwake;

    use super::super::awake::Awake as Control;
    use super::super::Power;
    use super::*;

    fn held(control: &Control<System>) -> bool {
        control.state.lock().unwrap().held.is_some()
    }

    #[test]
    fn the_request_is_taken_while_an_agent_works_and_let_go() {
        let control = Control::new(System);
        control.set_mode(KeepAwake::Working);
        assert!(!held(&control));
        control.set_busy(true);
        assert!(held(&control));
        control.set_busy(false);
        assert!(!held(&control));
    }
}
