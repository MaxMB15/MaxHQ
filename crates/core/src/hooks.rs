// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The rules a webhook request meets before it runs a task: its
//! signature, the task's filters, the keys that drop repeats, and the
//! prompt its template fills in. None of it does I/O, so `maxd` brings
//! the request and keeps what comes of it.

use std::collections::BTreeMap;

pub mod dedupe;
pub mod filter;
pub mod hold;
pub mod template;
pub mod verify;

/// A webhook request as the rules read it.
#[derive(Clone, Copy, Debug)]
pub struct Incoming<'a> {
    /// Its method, such as `POST`.
    pub method: &'a str,
    /// Its path, without the query.
    pub path: &'a str,
    /// Its query string, without the `?`.
    pub query: &'a str,
    /// Its headers, by lowercase name.
    pub headers: &'a BTreeMap<String, String>,
    /// Its body as text.
    pub body: &'a str,
}
