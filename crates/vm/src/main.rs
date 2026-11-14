// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `maxd-vm`: runs one macOS test VM for `maxd`. See the library for what
//! the two say to each other.

#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    mac::main()
}

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    eprintln!("maxd-vm runs only on macOS");
    std::process::ExitCode::FAILURE
}
