// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The dispatcher's guide tools: a search through MaxHQ's guide, which maxd
//! carries for its own version, and one section of it by its id. Both only
//! read.

use serde_json::{json, Value};

use super::agents::text;
use super::{fields, Call, Tool};

/// What the dispatcher reads about the guide, after its main
/// instructions.
pub const INSTRUCTIONS: &str = "The user may also ask how to do something in MaxHQ or \
    where to find something, such as a setting. Answer from MaxHQ's guide, not from memory: \
    call search_guide with the user's words, then read_guide on the section that fits when \
    its excerpt isn't enough. Answer in a few short steps in the guide's words, and end \
    with the section's link as a Markdown link, such as \
    [Open the Editor settings](maxhq://settings/editor), which opens that view when the user \
    clicks it. Fill a link's braces, such as {project} in maxhq://project/{project}, with \
    the id list_targets or list_agents gives, or leave that link out. When the guide has \
    nothing on it, say so rather than guess. A question needs no plan.";

/// The longest search the guide takes, in characters.
pub const MAX_QUERY: usize = 200;

/// A short title for each guide tool.
pub(super) fn title(tool: Tool) -> &'static str {
    match tool {
        Tool::SearchGuide => "Search the guide",
        Tool::ReadGuide => "Read a guide section",
        _ => unreachable!("not a guide tool"),
    }
}

/// What the dispatcher reads about each guide tool.
pub(super) fn description(tool: Tool) -> &'static str {
    match tool {
        Tool::SearchGuide => {
            "Searches MaxHQ's guide, the manual for this version, for how to do something or \
             where something is. Gives the few sections that fit best: each one's id, title, \
             a short excerpt and the links that open its views in the app."
        }
        Tool::ReadGuide => {
            "Reads one section of MaxHQ's guide, by the id search_guide gives: its whole text \
             and the links that open its views in the app."
        }
        _ => unreachable!("not a guide tool"),
    }
}

/// The JSON schema for each guide tool's arguments.
pub(super) fn input_schema(tool: Tool) -> Value {
    let (name, description) = match tool {
        Tool::SearchGuide => ("query", "What to look for, in the user's words."),
        Tool::ReadGuide => ("id", "The section's id, as search_guide gives it."),
        _ => unreachable!("not a guide tool"),
    };
    json!({
        "type": "object",
        "properties": {
            name: {"type": "string", "minLength": 1, "maxLength": MAX_QUERY, "description": description},
        },
        "required": [name],
        "additionalProperties": false,
    })
}

/// Reads a guide tool's arguments.
pub(super) fn parse(tool: Tool, arguments: &Value) -> Result<Call, String> {
    let args = fields(arguments)?;
    let (name, missing) = match tool {
        Tool::SearchGuide => ("query", "Give a `query`."),
        _ => ("id", "Give an `id`."),
    };
    let given = text(&args, name)?.ok_or(missing)?;
    if given.chars().count() > MAX_QUERY {
        return Err(format!("Keep `{name}` to {MAX_QUERY} characters."));
    }
    Ok(match tool {
        Tool::SearchGuide => Call::SearchGuide(given),
        Tool::ReadGuide => Call::ReadGuide(given),
        _ => unreachable!("not a guide tool"),
    })
}

/// The tools part of the guide's map: every agent tool and every
/// dispatcher tool, with its title, its description's first sentence and
/// the Settings page that governs it.
pub fn map_tools() -> Value {
    let dispatcher_only = Tool::DISPATCHER
        .into_iter()
        .filter(|tool| !Tool::ALL.contains(tool));
    let tools = Tool::ALL.into_iter().chain(dispatcher_only).map(|tool| {
        let agents = Tool::ALL.contains(&tool);
        let description = tool.description();
        let first = description
            .find(". ")
            .map_or(description, |end| &description[..=end]);
        json!({
            "id": format!("tool/{}", tool.name()),
            "name": tool.name(),
            "title": tool.title(),
            "description": first,
            "agents": agents,
            "dispatcher": Tool::DISPATCHER.contains(&tool),
            "link": if agents {
                "maxhq://settings/agent-tools"
            } else {
                "maxhq://settings/dispatcher"
            },
        })
    });
    Value::Array(tools.collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fails when the map's tools part no longer matches the tools. `pnpm
    /// guide:map` rewrites it, with the app's parts.
    #[test]
    fn generated_guide_map_tools_are_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../guide/map.json");
        let mut map: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let tools = map_tools();
        if std::env::var_os("MAXHQ_REGENERATE").is_some() {
            map["tools"] = tools;
            let text = serde_json::to_string_pretty(&map).unwrap() + "\n";
            std::fs::write(&path, text).unwrap();
        } else {
            assert!(
                map["tools"] == tools,
                "guide/map.json no longer matches the tools. Run `pnpm guide:map`."
            );
        }
    }

    #[test]
    fn a_search_and_a_read_take_one_trimmed_word_list() {
        assert_eq!(
            Tool::SearchGuide.parse(&json!({"query": "  forward a port "})),
            Ok(Call::SearchGuide("forward a port".into()))
        );
        assert_eq!(
            Tool::ReadGuide.parse(&json!({"id": "ports/forward"})),
            Ok(Call::ReadGuide("ports/forward".into()))
        );
        assert_eq!(
            Tool::SearchGuide.parse(&json!({})),
            Err("Give a `query`.".into())
        );
        assert!(Tool::ReadGuide
            .parse(&json!({"id": "x".repeat(MAX_QUERY + 1)}))
            .is_err());
    }

    #[test]
    fn only_the_dispatcher_gets_them() {
        for tool in [Tool::SearchGuide, Tool::ReadGuide] {
            assert!(Tool::DISPATCHER.contains(&tool));
            assert!(!Tool::ALL.contains(&tool));
            assert_eq!(Tool::named(tool.name()), Some(tool));
        }
    }
}
