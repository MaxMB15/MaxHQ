// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Fails when the checked-in JSON Schema no longer matches the Rust types,
//! for the wire messages and for the workflow standard. `pnpm bindings`
//! rewrites them.

use std::fs;
use std::path::Path;

use maxhq_protocol::schema;

#[test]
fn generated_schema_is_current() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workflow_dir = crate_dir
        .join("../..")
        .join(maxhq_workflow::schema::SCHEMA_DIR);
    let regenerate = std::env::var_os("MAXHQ_REGENERATE").is_some();
    let mut stale = Vec::new();
    let lists = [
        (crate_dir.join("schema"), schema::all()),
        (workflow_dir, maxhq_workflow::schema::all()),
    ];
    for (dir, name, schema) in lists
        .iter()
        .flat_map(|(dir, list)| list.iter().map(move |(name, schema)| (dir, *name, schema)))
    {
        let path = dir.join(name);
        let text = schema::render(schema);
        if regenerate {
            fs::create_dir_all(dir).unwrap();
            fs::write(&path, text).unwrap();
        } else if fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            stale.push(name);
        }
    }
    assert!(
        stale.is_empty(),
        "{stale:?} no longer match the Rust types. Run `pnpm bindings`."
    );
}
