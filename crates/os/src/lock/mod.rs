// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A lock that says a process holds a file, which the system lets go when
//! the process ends, however it ends.

use std::fs::{File, TryLockError};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

/// Takes the lock on `file` without waiting, or says another holds it.
/// Unlike [`File::try_lock`], it leaves the file readable to everyone else:
/// on Windows a lock also bars others from reading the bytes it covers, so
/// there it covers one byte far past any content, and a holder can still
/// write into the file what the others should read.
pub fn try_hold(file: &File) -> Result<(), TryLockError> {
    imp::try_hold(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::{Read, Write};

    #[test]
    fn a_held_file_bars_a_second_holder_and_stays_readable() {
        let dir = std::env::temp_dir().join(format!("maxhq-os-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("held.lock");
        let open = || {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .unwrap()
        };

        let mut first = open();
        try_hold(&first).unwrap();
        first.set_len(0).unwrap();
        write!(first, "4242").unwrap();
        first.flush().unwrap();

        let mut second = open();
        assert!(matches!(try_hold(&second), Err(TryLockError::WouldBlock)));
        assert!(matches!(second.try_lock(), Err(TryLockError::WouldBlock)));
        let mut text = String::new();
        second.read_to_string(&mut text).unwrap();
        assert_eq!(text, "4242");

        drop(first);
        try_hold(&second).unwrap();
        drop(second);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
