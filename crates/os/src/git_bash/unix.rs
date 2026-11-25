// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::path::PathBuf;

/// Git for Windows is Windows only.
pub fn find() -> Option<PathBuf> {
    None
}
