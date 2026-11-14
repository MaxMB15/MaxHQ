// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Passes the Rust target to the build, so a hello can say which binary a
//! device needs.

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=MAXD_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
