// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Wire types shared by `maxd` and every MaxHQ client.
//!
//! The Rust types here are the only definition. Two copies are generated from
//! them and checked in, and a test fails when either is stale:
//!
//! - JSON Schema under `schema/`, for clients not written in Rust.
//! - TypeScript in `packages/shared/src/bindings.ts`, through tauri-specta,
//!   so the app's commands and these types share one file.
//!
//! Decision A-7 has TypeScript generated from the JSON Schema. Going through
//! specta instead keeps one generator for the app's IPC and the protocol
//! (TS-15). The schema is still the contract for Swift.
//!
//! Field names stay snake_case on the wire and in TypeScript, so a field has
//! one name everywhere.

#![warn(missing_docs)]

pub mod acp;
pub mod admission;
pub mod agents;
pub mod attention;
pub mod context;
pub mod dispatch;
pub mod find;
pub mod frame;
pub mod guest;
pub mod hosts;
pub mod ids;
pub mod leases;
pub mod link;
pub mod message;
pub mod model;
pub mod netaccess;
pub mod notes;
pub mod phones;
pub mod processes;
pub mod pulls;
pub mod schema;
pub mod secret;
pub mod skills;
pub mod state;
pub mod tasks;
pub mod terminal;
pub mod time;
pub mod timeline;
mod tolerant;
pub mod workflow;
pub mod yaml;

pub use message::{ClientMessage, ProtocolRange, ServerMessage};
pub use secret::Secret;

// When to bump which:
//
// - Additive: a new message, entity kind, change, timeline event, item body,
//   outcome or error code, or a new field with `#[serde(default)]` and a doc
//   line saying what an older peer leaving it out reads as. Bump
//   PROTOCOL_VERSION only. An older peer skips or stands in for what it
//   can't read (see `tolerant`), and a new client message returns the new
//   version from `ClientMessage::since`, so the app won't send it to an older
//   `maxd`.
// - Breaking: a renamed, retyped or removed field, a new field without a
//   default, or a new variant in any other enum. Bump OLDEST_PROTOCOL_VERSION
//   too. A new variant inside an entity drops the whole entity on an older
//   client, so it counts as breaking unless that's fine.
//
// The app shows when a device's `maxd` is newer or too old.

/// Protocol version this build speaks. Bump it for any change a peer on the
/// previous version can't read.
pub const PROTOCOL_VERSION: u32 = 44;

/// Oldest protocol version this build still speaks.
pub const OLDEST_PROTOCOL_VERSION: u32 = 20;
