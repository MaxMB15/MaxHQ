// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::process::{Command, Stdio};

use super::trimmed;

pub fn name() -> Option<String> {
    let computer = if cfg!(target_os = "macos") {
        output("/usr/sbin/scutil", &["--get", "ComputerName"])
    } else {
        None
    };
    computer.as_deref().and_then(trimmed).or_else(host_name)
}

/// The host name, from the kernel or the `hostname` command.
fn host_name() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| output("hostname", &[]))
        .as_deref()
        .and_then(trimmed)
}

/// What a command printed when it succeeded.
fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}
