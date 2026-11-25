// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Tests of real holders running a fake agent CLI.
//!
//! This binary plays five parts, picked by its first argument. `__hold` is
//! the holder, `__child` is the fake CLI, `__spawner` is a process that
//! starts a holder and waits to be killed, `__sleep` is a process the fake
//! CLI leaves behind, and anything else runs the tests. Each test has its
//! own folders under the system temp folder, and only ever signals
//! processes it started.
//!
//! The tests whose holders act alike everywhere are in `common.rs`, what
//! the terminal tests share is in `term.rs`, and each OS adds its own in
//! `unix.rs` or `windows.rs`.

use std::ffi::OsString;
use std::process::ExitCode;

#[macro_use]
#[path = "holder/common.rs"]
mod common;
#[path = "holder/term.rs"]
mod term;

#[cfg(unix)]
#[path = "holder/unix.rs"]
mod unix;
#[cfg(unix)]
use unix as imp;

#[cfg(windows)]
#[path = "holder/windows.rs"]
mod windows;
#[cfg(windows)]
use windows as imp;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match args.first().and_then(|first| first.to_str()) {
        Some("__hold") => maxhq_hold::run_holder(&args[1..]),
        Some("__child") => common::fake_child(&args[1..]),
        Some("__spawner") => imp::spawner(&args[1..]),
        Some(common::SLEEP) => common::sleep(),
        _ => common::run_tests(&args, imp::TESTS, imp::IGNORED),
    }
}
