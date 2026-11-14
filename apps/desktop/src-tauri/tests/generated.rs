// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Fails when `packages/shared/src/bindings.ts` or
//! `apps/desktop/src/data/tauri-commands.ts` no longer matches the Rust
//! types. Run `pnpm bindings` to write them again.

use std::path::{Path, PathBuf};

use maxhq_desktop_lib::bindings::{self, COMMANDS_PATH, TYPES_PATH};

fn path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

#[test]
fn generated_bindings_are_current() {
    // The exporter only writes to a file, so export to a scratch file and
    // split what it wrote.
    let scratch = std::env::temp_dir().join(format!(
        "maxhq-bindings-{}-{}.ts",
        std::process::id(),
        std::thread::current()
            .name()
            .unwrap_or("test")
            .replace("::", "-"),
    ));
    maxhq_desktop_lib::specta_builder()
        .export(maxhq_desktop_lib::typescript(), &scratch)
        .expect("export the bindings");
    let full = std::fs::read_to_string(&scratch).expect("read the fresh bindings");
    let _ = std::fs::remove_file(&scratch);
    let split = bindings::split(&full).expect("split the bindings");

    let files = [(TYPES_PATH, split.types), (COMMANDS_PATH, split.commands)];
    if std::env::var_os("MAXHQ_REGENERATE").is_some() {
        for (relative, text) in files {
            std::fs::write(path(relative), text).expect("write the bindings");
        }
        return;
    }
    for (relative, fresh) in files {
        let path = path(relative);
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            fresh == committed,
            "{} is out of date. Run `pnpm bindings` and commit the result.",
            path.display(),
        );
    }
}
