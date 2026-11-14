// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Codex's `model/list` pages, as both the driver and the usage check read
//! them.

use maxhq_protocol::model::ModelChoice;
use serde_json::Value;

/// Adds the models one page of `model/list` shows to `into`, skipping
/// hidden ones. Returns the cursor for the next page, or none after the
/// last.
pub fn page(result: &Value, into: &mut Vec<ModelChoice>) -> Option<String> {
    for model in result
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if model.get("hidden").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(id) = model
            .get("model")
            .or_else(|| model.get("id"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let label = model
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or(id);
        into.push(ModelChoice {
            id: id.to_owned(),
            label: label.to_owned(),
            default: model.get("isDefault").and_then(Value::as_bool) == Some(true),
            efforts: model
                .get("supportedReasoningEfforts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|one| one.get("reasoningEffort").and_then(Value::as_str))
                .map(str::to_owned)
                .collect(),
            default_effort: model
                .get("defaultReasoningEffort")
                .and_then(Value::as_str)
                .map(str::to_owned),
        });
    }
    result
        .get("nextCursor")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_model_lists_its_efforts_and_the_one_it_uses_by_default() {
        let result = json!({"data": [
            {"model": "gpt-6", "displayName": "GPT-6", "isDefault": true,
             "supportedReasoningEfforts": [
                {"reasoningEffort": "low", "description": "Fast"},
                {"reasoningEffort": "medium", "description": "Balanced"},
                {"reasoningEffort": "ultra", "description": "Newer than us"}],
             "defaultReasoningEffort": "medium"},
            {"model": "gpt-6-mini", "supportedReasoningEfforts": []},
            {"model": "secret", "hidden": true},
        ], "nextCursor": null});
        let mut models = Vec::new();
        assert_eq!(page(&result, &mut models), None);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].efforts, ["low", "medium", "ultra"]);
        assert_eq!(models[0].default_effort.as_deref(), Some("medium"));
        assert!(models[1].efforts.is_empty());
        assert_eq!(models[1].default_effort, None);
        assert_eq!(models[1].label, "gpt-6-mini");
    }
}
