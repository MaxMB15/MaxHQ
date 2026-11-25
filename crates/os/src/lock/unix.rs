// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::fs::{File, TryLockError};

/// `flock` bars no one from reading, so the whole file's lock does.
pub fn try_hold(file: &File) -> Result<(), TryLockError> {
    file.try_lock()
}
