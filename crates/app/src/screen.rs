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

use thiserror::Error;

use crate::registry::{Registry, RegistryError};

/// The setting the switch is saved under.
const SETTING: &str = "screen_on";

/// What the system lists as the reason for the holds: Activity Monitor
/// and `pmset -g assertions` on macOS, `systemd-inhibit --list` and the
/// desktop on Linux.
#[cfg(any(target_os = "macos", target_os = "linux"))]
const REASON: &std::ffi::CStr = c"MaxHQ is keeping the screen on";

/// The holds while it's on: one keeps the display on, the other keeps the
/// Mac from sleeping when idle. Dropping them lets go.
#[cfg(target_os = "macos")]
type Held = [maxhq_core::power::Assertion; 2];
/// The desktop's and logind's holds. Dropping them lets go.
#[cfg(target_os = "linux")]
type Held = linux::Inhibitors;
/// Other systems never hold anything.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
type Held = std::convert::Infallible;

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
    held: Mutex<Option<Held>>,
}

impl ScreenOn {
    /// Whether this system can keep its screen on.
    pub const SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "linux"));

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

/// Takes macOS's two holds.
#[cfg(target_os = "macos")]
fn take() -> Result<Held, ScreenError> {
    use maxhq_core::power::{Assertion, Prevent};

    let take = |prevent| {
        Assertion::take(prevent, REASON).map_err(|error| ScreenError::Refused(error.to_string()))
    };
    Ok([take(Prevent::DisplaySleep)?, take(Prevent::IdleSleep)?])
}

#[cfg(target_os = "linux")]
use linux::take;

/// Other systems can't take anything.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn take() -> Result<Held, ScreenError> {
    Err(ScreenError::Unsupported)
}

/// Linux has no single switch for this, so the sun asks two services, the
/// way macOS takes two assertions. The desktop's screen saver keeps the
/// screen from dimming, blanking and locking, and logind keeps the
/// computer from sleeping when idle. Both answer over D-Bus, and each hold
/// also ends when the app's connection to it closes, so a crash can't
/// leave the screen on.
#[cfg(target_os = "linux")]
mod linux {
    use std::time::Duration;

    use dbus::arg::OwnedFd;
    use dbus::blocking::Connection;

    use super::{ScreenError, REASON};

    /// How long each service has to answer. They're local and answer at
    /// once, but the switch waits on them, so a stuck one mustn't hang it.
    const TIMEOUT: Duration = Duration::from_secs(2);

    /// The name the services list the holds under.
    const APP: &str = "MaxHQ";

    /// What the switch holds while it's on. Either half may be missing,
    /// when its service isn't there or said no, but not both.
    pub struct Inhibitors {
        _saver: Option<SaverHold>,
        /// The descriptor logind handed out. Closing it lets go.
        _logind: Option<OwnedFd>,
    }

    /// Takes both holds. One that fails is only logged, since the other
    /// still does part of the job. Both failing is an error.
    pub fn take() -> Result<Inhibitors, ScreenError> {
        let reason = REASON.to_string_lossy();
        let (saver, logind) = either(SaverHold::take(&reason), logind(&reason))?;
        Ok(Inhibitors {
            _saver: saver,
            _logind: logind,
        })
    }

    /// Keeps what worked of the two holds and logs what didn't, or fails
    /// when neither worked.
    fn either<A, B>(
        saver: Result<A, dbus::Error>,
        logind: Result<B, dbus::Error>,
    ) -> Result<(Option<A>, Option<B>), ScreenError> {
        match (saver, logind) {
            (Err(saver), Err(logind)) => {
                tracing::warn!(%saver, %logind, "neither the screen saver nor logind took a hold");
                Err(ScreenError::Refused(
                    "Neither the desktop's screen saver nor logind answered".into(),
                ))
            }
            (saver, logind) => {
                if let Err(error) = &saver {
                    tracing::info!(%error, "no screen saver took a hold, so the screen may still dim");
                }
                if let Err(error) = &logind {
                    tracing::info!(%error, "logind took no hold, so the computer may still sleep when idle");
                }
                Ok((saver.ok(), logind.ok()))
            }
        }
    }

    /// Asks logind on the system bus to block idling and sleep. The hold
    /// lasts while its descriptor is open, so the connection can close.
    fn logind(reason: &str) -> Result<OwnedFd, dbus::Error> {
        let bus = Connection::new_system()?;
        let (fd,): (OwnedFd,) = bus
            .with_proxy("org.freedesktop.login1", "/org/freedesktop/login1", TIMEOUT)
            .method_call(
                "org.freedesktop.login1.Manager",
                "Inhibit",
                ("idle:sleep", APP, reason, "block"),
            )?;
        Ok(fd)
    }

    /// Which screen saver took the hold. Most desktops answer on the
    /// freedesktop name. GNOME doesn't, but its session manager takes the
    /// same hold.
    #[derive(Clone, Copy)]
    enum Saver {
        Freedesktop,
        Gnome,
    }

    impl Saver {
        /// The bus name, object path and interface it answers on.
        fn address(self) -> (&'static str, &'static str, &'static str) {
            match self {
                Self::Freedesktop => (
                    "org.freedesktop.ScreenSaver",
                    "/org/freedesktop/ScreenSaver",
                    "org.freedesktop.ScreenSaver",
                ),
                Self::Gnome => (
                    "org.gnome.SessionManager",
                    "/org/gnome/SessionManager",
                    "org.gnome.SessionManager",
                ),
            }
        }

        /// Asks for the hold and returns its cookie.
        fn inhibit(self, bus: &Connection, reason: &str) -> Result<u32, dbus::Error> {
            /// GNOME's flag for keeping the session from going idle.
            const IDLE: u32 = 8;

            let (name, path, interface) = self.address();
            let proxy = bus.with_proxy(name, path, TIMEOUT);
            let (cookie,): (u32,) = match self {
                Self::Freedesktop => proxy.method_call(interface, "Inhibit", (APP, reason))?,
                // There's no window to name, so the window id is 0.
                Self::Gnome => {
                    proxy.method_call(interface, "Inhibit", (APP, 0_u32, reason, IDLE))?
                }
            };
            Ok(cookie)
        }

        /// Lets the hold with `cookie` go.
        fn release(self, bus: &Connection, cookie: u32) -> Result<(), dbus::Error> {
            let (name, path, interface) = self.address();
            let method = match self {
                Self::Freedesktop => "UnInhibit",
                Self::Gnome => "Uninhibit",
            };
            bus.with_proxy(name, path, TIMEOUT)
                .method_call(interface, method, (cookie,))
        }
    }

    /// A screen saver's hold, with the session bus connection that asked
    /// for it. The saver lets go when that connection closes too.
    struct SaverHold {
        bus: Connection,
        saver: Saver,
        cookie: u32,
    }

    impl SaverHold {
        /// Asks the freedesktop screen saver, or GNOME's when the first
        /// isn't there.
        fn take(reason: &str) -> Result<Self, dbus::Error> {
            let bus = Connection::new_session()?;
            let (saver, cookie) = match Saver::Freedesktop.inhibit(&bus, reason) {
                Ok(cookie) => (Saver::Freedesktop, cookie),
                Err(error) => {
                    tracing::debug!(%error, "no freedesktop screen saver, so asking GNOME's");
                    (Saver::Gnome, Saver::Gnome.inhibit(&bus, reason)?)
                }
            };
            Ok(Self { bus, saver, cookie })
        }
    }

    impl Drop for SaverHold {
        fn drop(&mut self) {
            // Closing the connection would end the hold anyway. Asking
            // first ends it now, rather than when the bus notices.
            if let Err(error) = self.saver.release(&self.bus, self.cookie) {
                tracing::debug!(%error, "the screen saver didn't take back its hold");
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn refused() -> dbus::Error {
            dbus::Error::new_failed("not here")
        }

        #[test]
        fn one_hold_is_enough() {
            let held = either::<u8, u8>(Ok(1), Err(refused())).unwrap();
            assert_eq!(held, (Some(1), None));
            let held = either::<u8, u8>(Err(refused()), Ok(2)).unwrap();
            assert_eq!(held, (None, Some(2)));
            let held = either::<u8, u8>(Ok(1), Ok(2)).unwrap();
            assert_eq!(held, (Some(1), Some(2)));
        }

        #[test]
        fn no_hold_at_all_is_refused() {
            assert!(matches!(
                either::<u8, u8>(Err(refused()), Err(refused())),
                Err(ScreenError::Refused(_))
            ));
        }
    }
}

// Linux's tests sit in its own module, since a hold there needs a desktop
// session to answer.
#[cfg(all(test, not(target_os = "linux")))]
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

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
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
