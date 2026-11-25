// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::io;
use std::path::Path;

/// A junction swap comes with the Windows update layout.
pub fn point(_link: &Path, _target: &Path) -> io::Result<()> {
    Err(crate::unsupported("Moving a link to a new version"))
}
