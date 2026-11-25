// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::path::PathBuf;

use maxhq_protocol::model::WindowsShell;

/// The user's shell: `$SHELL` when it's an absolute path, or zsh on macOS
/// and bash elsewhere.
pub(super) fn login_shell() -> PathBuf {
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|shell| shell.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(if cfg!(target_os = "macos") {
                "/bin/zsh"
            } else {
                "/bin/bash"
            })
        })
}

/// The user's shell, whatever the Windows setting says.
pub(super) fn pick_shell(_choice: WindowsShell) -> (PathBuf, bool) {
    (login_shell(), false)
}
