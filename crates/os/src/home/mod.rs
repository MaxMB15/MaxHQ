// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The user's home folder and name.

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
}
