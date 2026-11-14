// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Observers watch the agents on this device and report them to the hub.
//! Each one owns the agents it finds and replaces them as a set, so an
//! agent that ends disappears on the next pass. A session you started in a
//! terminal on this device stays as an ended agent instead, which MaxHQ can
//! continue.

pub mod claude;
pub mod codex;
pub mod incus;
