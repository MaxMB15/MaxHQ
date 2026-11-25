// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What this computer is called, for the name a device shows in the app
//! and the label beside its app key.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

/// This computer's name, trimmed, or none when the system gives none. On
/// macOS it's the name you set in System Settings, then the host name. On
/// Linux it's the kernel's host name, or what the `hostname` command
/// prints when the kernel's can't be read. On Windows it's the DNS host
/// name, which is what `hostname` prints there.
pub fn name() -> Option<String> {
    imp::name()
}

/// A name with the space and newline around it cut, or none when nothing
/// is left.
fn trimmed(raw: &str) -> Option<String> {
    let name = raw.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_computer_has_a_name_that_stays_put() {
        // macOS asks scutil, a child process.
        let _forks = crate::forks();
        let first = name().expect("this computer has a name");
        assert_eq!(first.trim(), first);
        assert!(!first.is_empty());
        assert_eq!(name(), Some(first));
    }

    #[test]
    fn a_name_is_trimmed_and_an_empty_one_is_none() {
        assert_eq!(
            trimmed(" Dev's MacBook Pro\n").as_deref(),
            Some("Dev's MacBook Pro")
        );
        assert_eq!(trimmed(" \n"), None);
        assert_eq!(trimmed(""), None);
    }
}
