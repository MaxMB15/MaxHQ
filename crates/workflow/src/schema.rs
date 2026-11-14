// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The standard's JSON Schemas, generated from the types and checked in.

use schemars::{schema_for, Schema};

use crate::descriptor::Descriptor;
use crate::reference::{EventLine, RunFile};

/// Where the schemas are checked in, from the repository's root.
pub const SCHEMA_DIR: &str = "spec/workflow/v1";

/// Where the schemas will be served, ending in `/`.
pub const SCHEMA_BASE: &str = "https://schemas.example.com/maxhq/workflow/v1/";

/// Every schema, with the file name it's checked in under in
/// [`SCHEMA_DIR`].
pub fn all() -> Vec<(&'static str, Schema)> {
    let mut schemas = vec![
        ("descriptor.schema.json", schema_for!(Descriptor)),
        ("run.schema.json", schema_for!(RunFile)),
        ("event.schema.json", schema_for!(EventLine)),
    ];
    for (name, schema) in &mut schemas {
        schema.insert("$id".to_owned(), format!("{SCHEMA_BASE}{name}").into());
    }
    schemas
}
