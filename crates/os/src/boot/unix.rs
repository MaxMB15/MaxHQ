// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::process::{Command, Stdio};

pub fn id() -> Option<String> {
    if cfg!(target_os = "macos") {
        let out = Command::new("/usr/sbin/sysctl")
            .args(["-n", "kern.bootsessionuuid"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()
    }
}
