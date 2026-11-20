// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! How each permission mode reads as Codex's approval policy, reviewer and
//! sandbox, and back. Every mode but full access keeps the sandbox to the
//! workspace with no network. The dispatcher runs in auto mode, so its
//! sandbox writes only its own folder and Codex's reviewer judges each
//! command that wants out of it.

use maxhq_protocol::model::PermissionMode;
use serde_json::{json, Map, Value};

/// One mode's settings, as `turn/start` and `thread/settings/update` take
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// The `approvalPolicy`, such as `on-request`.
    pub approval: &'static str,
    /// The `approvalsReviewer`: `user`, or `auto_review` for Codex's own
    /// reviewer.
    pub reviewer: &'static str,
    /// Whether the sandbox is off.
    pub full_access: bool,
}

/// The dispatcher's settings, whatever its mode: auto mode's. It reads
/// anywhere, writes only in its folder, which is its thread's `cwd`, and
/// runs a command outside the sandbox only when Codex's reviewer allows it.
pub const DISPATCHER: Policy = Policy {
    approval: "on-request",
    reviewer: "auto_review",
    full_access: false,
};

/// The settings for `mode`, or `None` for plan mode, which Codex lacks.
pub fn policy(mode: PermissionMode) -> Option<Policy> {
    let (approval, reviewer, full_access) = match mode {
        PermissionMode::Ask => ("untrusted", "user", false),
        PermissionMode::AcceptEdits => ("on-request", "user", false),
        PermissionMode::Auto => ("on-request", "auto_review", false),
        PermissionMode::FullAccess => ("never", "user", true),
        PermissionMode::Plan => return None,
    };
    Some(Policy {
        approval,
        reviewer,
        full_access,
    })
}

impl Policy {
    /// The `sandboxPolicy` object for a turn or a settings change.
    pub fn sandbox_policy(&self) -> Value {
        if self.full_access {
            return json!({ "type": "dangerFullAccess" });
        }
        json!({
            "type": "workspaceWrite",
            "writableRoots": [],
            "networkAccess": false,
            "excludeTmpdirEnvVar": false,
            "excludeSlashTmp": false,
        })
    }

    /// The `sandbox` mode name `thread/start` and `thread/resume` take.
    pub fn sandbox_mode(&self) -> &'static str {
        if self.full_access {
            "danger-full-access"
        } else {
            "workspace-write"
        }
    }

    /// Adds the fields `turn/start` and `thread/settings/update` take.
    pub fn add_to_turn(&self, params: &mut Map<String, Value>) {
        params.insert("approvalPolicy".into(), self.approval.into());
        params.insert("approvalsReviewer".into(), self.reviewer.into());
        params.insert("sandboxPolicy".into(), self.sandbox_policy());
    }

    /// Adds the fields `thread/start` and `thread/resume` take.
    pub fn add_to_thread(&self, params: &mut Map<String, Value>) {
        params.insert("approvalPolicy".into(), self.approval.into());
        params.insert("approvalsReviewer".into(), self.reviewer.into());
        params.insert("sandbox".into(), self.sandbox_mode().into());
    }
}

/// The mode a thread runs in, from its approval policy, reviewer and
/// sandbox. The sandbox is a `sandboxPolicy` object or a mode name. Any
/// mix MaxHQ never sets gives `None`.
pub fn effective(approval: &Value, reviewer: &Value, sandbox: &Value) -> Option<PermissionMode> {
    let sandbox = sandbox
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| sandbox.as_str())?;
    let full_access = match sandbox {
        "workspaceWrite" | "workspace-write" => false,
        "dangerFullAccess" | "danger-full-access" => true,
        _ => return None,
    };
    let reviewer = reviewer.as_str().unwrap_or("user");
    let mode = match (approval.as_str()?, reviewer, full_access) {
        ("untrusted", "user", false) => PermissionMode::Ask,
        ("on-request", "user", false) => PermissionMode::AcceptEdits,
        ("on-request", "auto_review", false) => PermissionMode::Auto,
        ("never", _, true) => PermissionMode::FullAccess,
        _ => return None,
    };
    Some(mode)
}

/// The mode in a `threadSettings` object or a `thread/start` or
/// `thread/resume` result.
pub fn effective_in(settings: &Value) -> Option<PermissionMode> {
    let sandbox = settings
        .get("sandboxPolicy")
        .or_else(|| settings.get("sandbox"))?;
    effective(
        settings.get("approvalPolicy")?,
        settings.get("approvalsReviewer").unwrap_or(&Value::Null),
        sandbox,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODES: [PermissionMode; 4] = [
        PermissionMode::Ask,
        PermissionMode::AcceptEdits,
        PermissionMode::Auto,
        PermissionMode::FullAccess,
    ];

    #[test]
    fn every_mode_but_plan_reads_back_as_itself() {
        for mode in MODES {
            let policy = policy(mode).unwrap();
            let mut turn = Map::new();
            policy.add_to_turn(&mut turn);
            assert_eq!(effective_in(&Value::Object(turn)), Some(mode));
            let mut thread = Map::new();
            policy.add_to_thread(&mut thread);
            assert_eq!(effective_in(&Value::Object(thread)), Some(mode));
        }
        assert_eq!(policy(PermissionMode::Plan), None);
    }

    #[test]
    fn the_dispatcher_writes_only_its_folder_and_asks_the_reviewer() {
        let mut turn = Map::new();
        DISPATCHER.add_to_turn(&mut turn);
        assert_eq!(turn["sandboxPolicy"]["type"], "workspaceWrite");
        assert_eq!(turn["sandboxPolicy"]["writableRoots"], json!([]));
        assert_eq!(turn["sandboxPolicy"]["networkAccess"], false);
        assert_eq!(turn["approvalPolicy"], "on-request");
        assert_eq!(turn["approvalsReviewer"], "auto_review");
        assert_eq!(
            effective_in(&Value::Object(turn)),
            Some(PermissionMode::Auto)
        );
        let mut thread = Map::new();
        DISPATCHER.add_to_thread(&mut thread);
        assert_eq!(thread["sandbox"], "workspace-write");
    }

    #[test]
    fn other_mixes_are_unknown() {
        let read_only = json!({
            "approvalPolicy": "on-request",
            "approvalsReviewer": "user",
            "sandboxPolicy": { "type": "readOnly" },
        });
        assert_eq!(effective_in(&read_only), None);
        let granular = json!({
            "approvalPolicy": { "granular": {} },
            "sandbox": "workspace-write",
        });
        assert_eq!(effective_in(&granular), None);
    }
}
