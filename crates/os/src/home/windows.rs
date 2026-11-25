// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::io;
use std::path::PathBuf;

/// `USERPROFILE`, then the profile folder Windows keeps for the user. `HOME`
/// is ignored: Git Bash and MSYS set it to a path of their own.
pub fn home_dir() -> Option<PathBuf> {
    std::env::home_dir().filter(|home| !home.as_os_str().is_empty())
}

pub fn user_name() -> Option<String> {
    std::env::var("USERNAME")
        .ok()
        .filter(|user| !user.is_empty())
}

pub fn user_sid() -> io::Result<String> {
    crate::win::sid_string(&crate::win::own_user()?)
}
