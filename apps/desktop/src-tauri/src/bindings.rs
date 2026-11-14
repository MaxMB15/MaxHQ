// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Splits the one file tauri-specta writes in two. The types and constants
//! have no Tauri in them, so they go to the shared package, which the phone
//! app reads too. The commands and events call into Tauri, so they go to the
//! desktop app alone and import the types they name.

use std::collections::BTreeSet;

/// Where `pnpm bindings` writes the types, relative to this crate.
pub const TYPES_PATH: &str = "../../../packages/shared/src/bindings.ts";

/// Where `pnpm bindings` writes the Tauri commands and events, relative to
/// this crate.
pub const COMMANDS_PATH: &str = "../src/data/tauri-commands.ts";

/// The module the commands file imports the types from.
const TYPES_MODULE: &str = "@maxhq/shared/bindings";

// The section markers tauri-specta writes, in the order it writes them.
const COMMANDS: &str = "/** Commands */";
const CONSTANTS: &str = "/* Constants */";
const RUNTIME: &str = "/* Tauri Specta runtime */";

/// The two files, as text.
#[derive(Debug, PartialEq, Eq)]
pub struct Split {
    /// The constants and types, for [`TYPES_PATH`].
    pub types: String,
    /// The commands, events and their runtime, for [`COMMANDS_PATH`].
    pub commands: String,
}

/// Splits the exporter's output. Fails when a section marker is missing,
/// which means a tauri-specta update changed its layout.
pub fn split(full: &str) -> Result<Split, String> {
    let find = |marker: &str| {
        full.find(marker)
            .ok_or_else(|| format!("the bindings have no `{marker}` section"))
    };
    let commands_at = find(COMMANDS)?;
    let constants_at = find(CONSTANTS)?;
    let runtime_at = find(RUNTIME)?;
    if !(commands_at < constants_at && constants_at < runtime_at) {
        return Err("the bindings sections are in an unexpected order".into());
    }

    // The header and the Tauri imports come before the commands. The header
    // is its comment lines.
    let prelude = &full[..commands_at];
    let header: String = prelude
        .lines()
        .take_while(|line| line.starts_with("//"))
        .map(|line| format!("{line}\n"))
        .collect();
    let imports = prelude[header.len()..].trim();
    let calls = full[commands_at..constants_at].trim_end();
    let types = full[constants_at..runtime_at].trim_end();
    let runtime = full[runtime_at..].trim_end();

    let exported: BTreeSet<&str> = types
        .lines()
        .filter_map(|line| line.strip_prefix("export type "))
        .filter_map(|rest| rest.split([' ', '<', '=']).next())
        .collect();
    let used = identifiers(&strip_comments(&format!("{calls}\n{runtime}")));
    let names: Vec<&str> = exported
        .into_iter()
        .filter(|name| used.contains(*name))
        .collect();
    let import_types = format!(
        "import type {{ {} }} from \"{TYPES_MODULE}\";",
        names.join(", ")
    );

    Ok(Split {
        types: format!("{header}\n{types}\n"),
        commands: format!("{header}\n{imports}\n\n{import_types}\n\n{calls}\n\n{runtime}\n"),
    })
}

// The text without its `//` and `/* */` comments, so a type a comment
// mentions isn't taken for one the code uses.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("/*") {
            rest = after.find("*/").map_or("", |end| &after[end + 2..]);
        } else if let Some(after) = rest.strip_prefix("//") {
            rest = after.find('\n').map_or("", |end| &after[end..]);
        } else {
            let mut chars = rest.chars();
            out.extend(chars.next());
            rest = chars.as_str();
        }
    }
    out
}

fn identifiers(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "// Regenerate.\n// Generated.\n\n\
        import { invoke as __TAURI_INVOKE } from \"@tauri-apps/api/core\";\n\n\
        /** Commands */\nexport const commands = {\n\
        \t/** Takes a `Hidden` and a `Request`. */\n\
        \trequest: (request: Request) => typedError<Response, CommandError>(__TAURI_INVOKE(\"request\", { request })),\n\
        };\n\n\
        /* Constants */\nexport const DEPTH = 3 as const;\n\n\
        /* Types */\nexport type Request = { a: Hidden };\n\n\
        export type Response = string;\n\n\
        export type Hidden = number;\n\n\
        export type CommandError = { kind: \"failed\" };\n\n\
        export type Unused<T> = T;\n\n\
        /* Tauri Specta runtime */\nasync function typedError<T, E>(result: Promise<T>) {}\n";

    #[test]
    fn the_types_file_has_no_tauri_in_it() {
        let split = split(SAMPLE).unwrap();
        assert!(split
            .types
            .starts_with("// Regenerate.\n// Generated.\n\n/* Constants */"));
        assert!(!split.types.contains("tauri"));
        assert!(!split.types.contains("commands"));
        assert!(split.types.contains("export type Unused<T> = T;"));
    }

    #[test]
    fn the_commands_file_imports_only_the_types_its_code_names() {
        let split = split(SAMPLE).unwrap();
        assert!(split.commands.contains(
            "import type { CommandError, Request, Response } from \"@maxhq/shared/bindings\";"
        ));
        assert!(split.commands.contains("@tauri-apps/api/core"));
        assert!(split.commands.contains("/* Tauri Specta runtime */"));
        assert!(!split.commands.contains("export type"));
    }

    #[test]
    fn a_missing_section_is_an_error() {
        assert!(split(&SAMPLE.replace("/* Constants */", "")).is_err());
    }
}
