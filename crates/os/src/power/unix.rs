// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::io;

#[derive(Debug)]
pub enum Request {}

pub fn take(_reason: &str) -> io::Result<Request> {
    Err(crate::unsupported("a Windows power request"))
}

pub fn on_charger() -> io::Result<bool> {
    Err(crate::unsupported("Windows's power status"))
}
