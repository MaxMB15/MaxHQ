// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Passes the Rust target to the build, so a hello can say which binary a
//! device needs, and embeds MaxHQ's guide, so the dispatcher answers from
//! the guide for this version.

use std::path::Path;

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=MAXD_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
    embed_guide();
}

/// Writes `guide.rs` to `OUT_DIR`: each page of guide/ by its name, and the
/// map, as `include_str!`s.
fn embed_guide() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let dir = Path::new(&manifest).join("../../guide");
    let dir = dir
        .canonicalize()
        .unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut pages: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
        .map(|entry| entry.expect("a guide entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect();
    pages.sort();
    let mut code = String::from("pub(super) const PAGES: &[(&str, &str)] = &[\n");
    for path in &pages {
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("a UTF-8 page name");
        code += &format!(
            "    ({name:?}, include_str!({:?})),\n",
            path.display().to_string()
        );
    }
    code += "];\n";
    code += &format!(
        "pub(super) const MAP: &str = include_str!({:?});\n",
        dir.join("map.json").display().to_string()
    );
    let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    std::fs::write(Path::new(&out).join("guide.rs"), code).expect("write guide.rs");
}
