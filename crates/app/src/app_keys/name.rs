// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What this computer is called, in the label a device shows beside its
//! app key: "MaxHQ on <name>".

/// This computer's name, as you set it in the system's settings, or its
/// host name.
pub(super) fn computer_name() -> String {
    system_name().unwrap_or_else(|| FALLBACK.to_owned())
}

/// What the label says when the system gives no name.
#[cfg(target_os = "macos")]
const FALLBACK: &str = "this Mac";
#[cfg(windows)]
const FALLBACK: &str = "this PC";
#[cfg(not(any(target_os = "macos", windows)))]
const FALLBACK: &str = "this computer";

/// The Mac's name from System Settings, or its host name.
#[cfg(target_os = "macos")]
fn system_name() -> Option<String> {
    use std::process::{Command, Stdio};
    let output = |program: &str, args: &[&str]| {
        let out = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        (out.status.success() && !text.is_empty()).then_some(text)
    };
    output("/usr/sbin/scutil", &["--get", "ComputerName"]).or_else(|| output("hostname", &[]))
}

/// The pretty name the system's settings give the computer, then its host
/// name. Both are read from files, so no process starts.
#[cfg(target_os = "linux")]
fn system_name() -> Option<String> {
    let read = |path: &str| std::fs::read_to_string(path).ok();
    read("/etc/machine-info")
        .as_deref()
        .and_then(pretty_hostname)
        .or_else(|| {
            let name = read("/proc/sys/kernel/hostname")?.trim().to_owned();
            (!name.is_empty()).then_some(name)
        })
}

/// The computer's name as Windows shows it.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn system_name() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|name| !name.trim().is_empty())
}

/// `PRETTY_HOSTNAME` in `/etc/machine-info`, an environment-style file
/// whose values may be quoted.
#[cfg_attr(
    not(target_os = "linux"),
    allow(dead_code, reason = "only Linux keeps a machine-info file")
)]
fn pretty_hostname(text: &str) -> Option<String> {
    let value = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("PRETTY_HOSTNAME="))?
        .trim();
    let unquoted = ['"', '\'']
        .iter()
        .find_map(|quote| {
            value
                .strip_prefix(*quote)
                .and_then(|rest| rest.strip_suffix(*quote))
        })
        .unwrap_or(value);
    let name = unquoted.replace("\\\"", "\"").replace("\\\\", "\\");
    (!name.trim().is_empty()).then(|| name.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pretty_name_reads_quoted_or_bare() {
        let file = "CHASSIS=laptop\nPRETTY_HOSTNAME=\"Sam's \\\"work\\\" laptop\"\n";
        assert_eq!(
            pretty_hostname(file).as_deref(),
            Some("Sam's \"work\" laptop")
        );
        assert_eq!(
            pretty_hostname("PRETTY_HOSTNAME=studio").as_deref(),
            Some("studio")
        );
        assert_eq!(
            pretty_hostname("PRETTY_HOSTNAME='the den'").as_deref(),
            Some("the den")
        );
    }

    #[test]
    fn a_missing_or_empty_pretty_name_falls_through() {
        assert_eq!(pretty_hostname("CHASSIS=desktop"), None);
        assert_eq!(pretty_hostname("PRETTY_HOSTNAME=\"\""), None);
        assert!(!computer_name().is_empty());
    }
}
