// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The user's home folder, name and SID.

use std::io;
use std::path::PathBuf;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

/// The user's home folder: `HOME` on Unix, so a test or a service that
/// sets it is followed, and the profile folder on Windows, where `HOME`
/// is usually unset.
pub fn home_dir() -> Option<PathBuf> {
    imp::home_dir()
}

/// The user's login name: `USER` on Unix and `USERNAME` on Windows.
pub fn user_name() -> Option<String> {
    imp::user_name()
}

/// The user's security ID in its `S-1-5-…` form, which names them on
/// Windows however their account is renamed. Unix has none.
pub fn user_sid() -> io::Result<String> {
    imp::user_sid()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_user_has_a_home() {
        let home = home_dir().expect("a home folder");
        assert!(home.is_absolute(), "{}", home.display());
    }

    /// Windows always sets `USERNAME`. A Unix service or container may
    /// start without `USER`, so only Windows checks it.
    #[cfg(windows)]
    #[test]
    fn this_user_has_a_name() {
        assert!(!user_name().expect("a user name").is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn this_user_has_a_sid() {
        let sid = user_sid().unwrap();
        assert!(sid.starts_with("S-1-5-"), "{sid}");
        assert_eq!(user_sid().unwrap(), sid);
    }

    #[cfg(unix)]
    #[test]
    fn unix_users_have_no_sid() {
        assert_eq!(user_sid().unwrap_err().kind(), io::ErrorKind::Unsupported);
    }
}
