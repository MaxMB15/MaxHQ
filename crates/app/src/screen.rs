// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The sun beside the header's title. While it's on, this Mac's screen
//! doesn't dim or turn off and the Mac doesn't sleep when idle, on the
//! charger or on battery. While it's off, the device's Keep awake setting
//! and macOS decide, as before. The app holds this itself, apart from
//! `maxd`, so it ends when the app quits. The switch is saved, so it's back
//! on when the app opens again. Closing the lid still sleeps the Mac.

use std::sync::{Mutex, MutexGuard, PoisonError};

use thiserror::Error;

use crate::registry::{Registry, RegistryError};

/// The setting the switch is saved under.
const SETTING: &str = "screen_on";

/// The holds while it's on: one keeps the display on, the other keeps the
/// Mac from sleeping when idle. Dropping them lets go.
#[cfg(target_os = "macos")]
type Held = [maxhq_core::power::Assertion; 2];
/// Other systems never hold anything.
#[cfg(not(target_os = "macos"))]
type Held = std::convert::Infallible;

/// Why the sun didn't change.
#[derive(Debug, Error)]
pub enum ScreenError {
    /// Only macOS can keep its screen on.
    #[error("this system can't keep its screen on")]
    Unsupported,
    /// macOS didn't take the hold.
    #[error("{0}")]
    Refused(String),
    /// The switch couldn't be saved, so it was left as it was.
    #[error(transparent)]
    Save(#[from] RegistryError),
}

/// Keeps this Mac's screen on while the sun is on.
#[derive(Default)]
pub struct ScreenOn {
    held: Mutex<Option<Held>>,
}

impl ScreenOn {
    /// Whether this system can keep its screen on.
    pub const SUPPORTED: bool = cfg!(target_os = "macos");

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

    fn lock(&self) -> MutexGuard<'_, Option<Held>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes the holds, or lets them go.
    #[cfg(target_os = "macos")]
    fn hold(&self, on: bool) -> Result<(), ScreenError> {
        use maxhq_core::power::{Assertion, Prevent};

        /// What Activity Monitor and `pmset -g assertions` show.
        const REASON: &std::ffi::CStr = c"MaxHQ is keeping the screen on";

        let mut held = self.lock();
        if !on {
            if held.take().is_some() {
                tracing::debug!("the screen may dim again");
            }
            return Ok(());
        }
        if held.is_none() {
            let take = |prevent| {
                Assertion::take(prevent, REASON)
                    .map_err(|error| ScreenError::Refused(error.to_string()))
            };
            *held = Some([take(Prevent::DisplaySleep)?, take(Prevent::IdleSleep)?]);
            tracing::debug!("keeping the screen on");
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    fn hold(&self, on: bool) -> Result<(), ScreenError> {
        if on {
            Err(ScreenError::Unsupported)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
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

    #[cfg(not(target_os = "macos"))]
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
