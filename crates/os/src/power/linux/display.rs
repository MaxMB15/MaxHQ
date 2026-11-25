// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Linux has no single switch for keeping the display on, so the hold asks
//! two services, the way macOS takes two assertions. The desktop's screen
//! saver keeps the screen from dimming, blanking and locking, and logind
//! keeps the computer from sleeping when idle. Both answer over D-Bus, and
//! each hold also ends when this process's connection to it closes, so a
//! crash can't leave the screen on.

use std::io;
use std::time::Duration;

use dbus::arg::OwnedFd;
use dbus::blocking::Connection;

/// How long each service has to answer. They're local and answer at once,
/// but the app's switch waits on them, so a stuck one mustn't hang it.
const TIMEOUT: Duration = Duration::from_secs(2);

/// The name the services list the holds under.
const APP: &str = "MaxHQ";

/// What the hold keeps. Either half may be missing, when its service isn't
/// there or said no, but not both.
pub struct Inhibitors {
    _saver: Option<SaverHold>,
    /// The descriptor logind handed out. Closing it lets go.
    _logind: Option<OwnedFd>,
}

/// Takes both holds. One that fails is only logged, since the other still
/// does part of the job. Both failing is an error.
pub fn take(reason: &str) -> io::Result<Inhibitors> {
    let (saver, logind) = either(SaverHold::take(reason), logind(reason))?;
    Ok(Inhibitors {
        _saver: saver,
        _logind: logind,
    })
}

/// Keeps what worked of the two holds and logs what didn't, or fails when
/// neither worked.
fn either<A, B>(
    saver: Result<A, dbus::Error>,
    logind: Result<B, dbus::Error>,
) -> io::Result<(Option<A>, Option<B>)> {
    match (saver, logind) {
        (Err(saver), Err(logind)) => {
            tracing::warn!(%saver, %logind, "neither the screen saver nor logind took a hold");
            Err(io::Error::other(
                "Neither the desktop's screen saver nor logind answered",
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

/// Asks logind on the system bus to block idling and sleep. The hold lasts
/// while its descriptor is open, so the connection can close.
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
/// freedesktop name. GNOME doesn't, but its session manager takes the same
/// hold.
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
            Self::Gnome => proxy.method_call(interface, "Inhibit", (APP, 0_u32, reason, IDLE))?,
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

/// A screen saver's hold, with the session bus connection that asked for
/// it. The saver lets go when that connection closes too.
struct SaverHold {
    bus: Connection,
    saver: Saver,
    cookie: u32,
}

impl SaverHold {
    /// Asks the freedesktop screen saver, or GNOME's when the first isn't
    /// there.
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
        // Closing the connection would end the hold anyway. Asking first
        // ends it now, rather than when the bus notices.
        if let Err(error) = self.saver.release(&self.bus, self.cookie) {
            tracing::debug!(%error, "the screen saver didn't take back its hold");
        }
    }
}

// A real hold needs a desktop session to answer, so only the choice is
// tested.
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
        let err = either::<u8, u8>(Err(refused()), Err(refused())).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Neither the desktop's screen saver nor logind answered"
        );
    }
}
