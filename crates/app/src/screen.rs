// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The sun beside the header's title. While it's on, this computer's
//! screen doesn't dim or turn off and the computer doesn't sleep when
//! idle, on the charger or on battery. While it's off, the device's Keep
//! awake setting and the system decide, as before. The app holds this
//! itself, apart from `maxd`, so it ends when the app quits. The switch is
//! saved, so it's back on when the app opens again. Closing the lid still
//! sleeps the computer.

use std::sync::{Mutex, MutexGuard, PoisonError};

use maxhq_os::power::{Awake, Keep};
use thiserror::Error;

use crate::registry::{Registry, RegistryError};

/// The setting the switch is saved under.
const SETTING: &str = "screen_on";

/// What the system lists as the reason for the holds: Activity Monitor
/// and `pmset -g assertions` on macOS, `systemd-inhibit --list` and the
/// desktop on Linux, `powercfg /requests` on Windows.
const REASON: &str = "MaxHQ is keeping the screen on";

/// Why the sun didn't change.
#[derive(Debug, Error)]
pub enum ScreenError {
    /// This system can't keep its screen on.
    #[error("this system can't keep its screen on")]
    Unsupported,
    /// The system didn't take the hold.
    #[error("{0}")]
    Refused(String),
    /// The switch couldn't be saved, so it was left as it was.
    #[error(transparent)]
    Save(#[from] RegistryError),
}

/// Keeps this computer's screen on while the sun is on.
#[derive(Default)]
pub struct ScreenOn {
    /// The hold while it's on, which keeps the display on and the
    /// computer from sleeping when idle. Dropping it lets go.
    held: Mutex<Option<Awake>>,
}

impl ScreenOn {
    /// Whether this system can keep its screen on.
    pub const SUPPORTED: bool = Keep::Display.supported();

    /// Starts with the switch saved in `registry`, off when there's none.
    pub fn start(registry: &Registry) -> Self {
        let screen = Self::default();
        let saved = match registry.setting::<bool>(SETTING) {
            Ok(saved) => saved.unwrap_or(false),
            Err(error) => {
                tracing::warn!(%error, "couldn't read whether to keep the screen on");
                false
            }
        };
        if saved
            && Self::SUPPORTED
            && let Err(error) = screen.hold(true)
        {
            tracing::warn!(%error, "couldn't keep the screen on");
        }
        screen
    }

    /// Whether the screen is held on now.
    pub fn is_on(&self) -> bool {
        self.lock().is_some()
    }

    /// Turns the sun on or off and saves it in `registry`. On a failure
    /// nothing changes.
    pub fn set(&self, on: bool, registry: &Registry) -> Result<(), ScreenError> {
        if !Self::SUPPORTED {
            return Err(ScreenError::Unsupported);
        }
        let was = self.is_on();
        self.hold(on)?;
        if let Err(error) = registry.set_setting(SETTING, &on) {
            // Puts back what was there, which it was able to take before.
            let _ = self.hold(was);
            return Err(error.into());
        }
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, Option<Awake>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes the holds, or lets them go.
    fn hold(&self, on: bool) -> Result<(), ScreenError> {
        let mut held = self.lock();
        if !on {
            if held.take().is_some() {
                tracing::debug!("the screen may dim again");
            }
            return Ok(());
        }
        if held.is_none() {
            *held = Some(take()?);
            tracing::debug!("keeping the screen on");
        }
        Ok(())
    }
}

/// Takes the display's hold.
fn take() -> Result<Awake, ScreenError> {
    Awake::take(Keep::Display, REASON).map_err(|error| ScreenError::Refused(error.to_string()))
}

// A hold on Linux needs a desktop session to answer, so the sun's test
// runs on macOS and Windows.
#[cfg(all(test, not(target_os = "linux")))]
mod tests {
    use super::*;

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn the_sun_holds_the_screen_and_comes_back_on_with_the_app() {
        let registry = Registry::in_memory().unwrap();
        let screen = ScreenOn::start(&registry);
        assert!(!screen.is_on());

        screen.set(true, &registry).unwrap();
        assert!(screen.is_on());
        // Turning it on again keeps the holds it has.
        screen.set(true, &registry).unwrap();
        assert!(screen.is_on());

        let reopened = ScreenOn::start(&registry);
        assert!(reopened.is_on());

        screen.set(false, &registry).unwrap();
        assert!(!screen.is_on());
        assert!(!ScreenOn::start(&registry).is_on());
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    #[test]
    fn other_systems_say_they_cant() {
        let registry = Registry::in_memory().unwrap();
        let screen = ScreenOn::start(&registry);
        assert!(matches!(
            screen.set(true, &registry),
            Err(ScreenError::Unsupported)
        ));
        assert!(!screen.is_on());
    }
}
