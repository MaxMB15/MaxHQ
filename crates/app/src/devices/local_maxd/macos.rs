// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! On macOS the service runs the `maxd` inside the app's bundle, which
//! stays put once the app is in an Applications folder, and the app's own
//! updates replace it there.

use std::io;
use std::path::{Path, PathBuf};

/// Why the app won't install `maxd` from where it runs now.
pub(super) const MISPLACED: &str =
    "Move MaxHQ to your Applications folder, open it from there, then start maxd again.";

/// Why `bundled` runs from somewhere a login service can't keep pointing
/// at: a copy macOS moved aside because the app was opened where it was
/// downloaded, or a disk image, which goes once you eject it.
pub(super) fn misplaced(bundled: &Path) -> Option<&'static str> {
    crate::app_update::moved_aside(bundled).map(|_| MISPLACED)
}

/// The service runs the bundled `maxd` itself, so nothing is copied.
pub(super) fn stage(bundled: &Path, _home: Option<&Path>) -> io::Result<PathBuf> {
    Ok(bundled.to_owned())
}

/// The program the service runs once the app installed `maxd`.
pub(super) fn installed(bundled: &Path, _home: Option<&Path>) -> Option<PathBuf> {
    Some(bundled.to_owned())
}
