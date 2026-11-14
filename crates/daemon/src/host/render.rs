// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `maxd host render <item> --spec <file>`: one file the root part
//! computes from the spec, printed. The NixOS module runs it at build
//! time, so its units load the same rules and run the same script that
//! `maxd host apply --system` writes elsewhere, and the module itself only
//! wires units and options. Each render checks the spec first, so a bad
//! spec fails the build.

use std::path::Path;

use maxhq_core::host_spec::HostSpec;

use super::system;

/// The items there are, as `maxd host render` names them.
pub const ITEMS: &[&str] = &["json", "prepare", "egress", "memory-low"];

/// Renders `item` from the spec at `path`, read as JSON when its name ends
/// in `.json` and as TOML otherwise.
pub fn render(item: &str, path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("couldn't read {}: {err}", path.display()))?;
    let json = path.extension().is_some_and(|ext| ext == "json");
    render_text(item, &text, json)
}

/// Renders `item` from the spec's text.
pub fn render_text(item: &str, text: &str, json: bool) -> Result<String, String> {
    let spec = if json {
        HostSpec::from_json(text)
    } else {
        HostSpec::from_toml(text)
    }
    .map_err(|err| err.to_string())?;
    let missing = |why: &str| format!("the spec has no {item}: {why}");
    match item {
        "json" => as_json(text, json),
        "prepare" => system::prepare_script(&spec)
            .map(|script| format!("{script}\n"))
            .ok_or_else(|| missing("it needs caps.parent and caps.manage_parent = true")),
        "egress" => system::egress_rules(&spec)
            .ok_or_else(|| missing("it needs incus.enable and incus.bridge")),
        "memory-low" => {
            system::memory_low(&spec).ok_or_else(|| missing("it needs caps.system_memory_low"))
        }
        _ => Err(format!(
            "there's no item {item:?}, only {}",
            ITEMS.join(", ")
        )),
    }
}

/// The spec as `/etc/maxhq/host.json`, which maxd reads back.
fn as_json(text: &str, json: bool) -> Result<String, String> {
    let value: serde_json::Value = if json {
        serde_json::from_str(text).map_err(|err| err.to_string())?
    } else {
        let table: toml::Table = toml::from_str(text).map_err(|err| err.to_string())?;
        serde_json::to_value(table).map_err(|err| err.to_string())?
    };
    serde_json::to_string_pretty(&value)
        .map(|text| format!("{text}\n"))
        .map_err(|err| err.to_string())
}
