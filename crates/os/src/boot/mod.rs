// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The boot this device is on, which changes each time it starts.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

/// The boot ID as the system gives it, untrimmed, or none when it can't be
/// read: `kern.bootsessionuuid` on macOS, the kernel's boot ID on Linux,
/// and the count of boots Windows keeps in its registry.
pub fn id() -> Option<String> {
    imp::id()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_boot_is_readable_and_stays_put() {
        // macOS asks sysctl, a child process.
        let _forks = crate::forks();
        let boot = id().expect("the boot ID is readable here");
        assert!(!boot.trim().is_empty());
        assert_eq!(id(), Some(boot));
    }
}
