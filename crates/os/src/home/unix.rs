// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::path::PathBuf;

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

pub fn user_name() -> Option<String> {
    std::env::var("USER").ok().filter(|user| !user.is_empty())
}
