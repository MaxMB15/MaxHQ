// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::fs;
use std::io;
use std::path::Path;

/// Points `link` at `target` through a symlink and a rename.
pub fn point(link: &Path, target: &Path) -> io::Result<()> {
    let temp = super::temp_for(link);
    let _ = fs::remove_file(&temp);
    std::os::unix::fs::symlink(target, &temp)?;
    fs::rename(&temp, link)
}
