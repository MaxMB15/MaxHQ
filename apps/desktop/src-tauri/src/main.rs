// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Without this, Windows opens a console window next to the app in release
// builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    maxhq_desktop_lib::run();
}
