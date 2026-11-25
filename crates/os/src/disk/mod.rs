// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! How much room a disk has left.

use std::io;
use std::path::Path;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

/// Bytes free for this user on the disk that holds the folder `dir`,
/// which has to exist.
pub fn free_space(dir: &Path) -> io::Result<u64> {
    imp::free_space(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_that_exists_has_room_and_one_that_doesnt_fails() {
        let temp = std::env::temp_dir();
        assert!(free_space(&temp).unwrap() > 0);
        assert!(free_space(&temp.join("maxhq-no-such-folder/a")).is_err());
    }
}
