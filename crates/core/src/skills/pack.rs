// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A project's devflow pack: a library source used whole, copied into
//! each checkout's `.devflow-vendor` and installed there by its own
//! `install.sh`. A marker in the copy names its tree and how the install
//! went, and a `.devflow-vendor` without one is someone else's. A repository
//! can commit a marker, so whether a tree has installed comes from maxd's
//! own note of the same fields, kept outside the checkout.

use maxhq_protocol::skills::{PackState, TreeHash};
use serde::{Deserialize, Serialize};

/// Where a checkout keeps its pack, from its root.
pub const PACK_DIR: &str = ".devflow-vendor";

/// The pack's installer, from the pack's root.
pub const INSTALLER: &str = "install.sh";

/// MaxHQ's marker in a pack copy, from the pack's root.
pub const MARKER: &str = ".maxhq-pack.json";

/// What the installer wants git to ignore before it runs, as paths from
/// the repo's root.
const IGNORED: [&str; 14] = [
    ".devflow-vendor/",
    ".devflow/",
    ".claude/",
    ".cursor/",
    ".codex/",
    ".github/skills/",
    ".github/agents/",
    ".github/copilot-instructions.md",
    ".vscode/mcp.json",
    "CLAUDE.md",
    "AGENTS.md",
    ".mcp.json",
    ".code-review-graph/",
    ".gitignore",
];

/// The lines `.git/info/exclude` needs before the installer runs.
pub fn exclude_lines() -> Vec<String> {
    IGNORED.iter().map(|path| format!("/{path}")).collect()
}

/// How the last install of a copy went.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ran {
    /// Copied, and the installer hasn't finished.
    Pending,
    /// The installer ran.
    Installed,
    /// The installer failed.
    Failed,
}

/// MaxHQ's marker in a pack copy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    /// The tree the copy holds.
    pub tree: TreeHash,
    /// How its install went.
    pub ran: Ran,
    /// The last lines of a failed install's log.
    #[serde(default)]
    pub lines: Vec<String>,
}

impl Marker {
    /// Where the copy stands.
    pub fn state(&self) -> PackState {
        let tree = self.tree.clone();
        match self.ran {
            Ran::Pending => PackState::Installing { tree },
            Ran::Installed => PackState::Installed { tree },
            Ran::Failed => PackState::Failed {
                tree,
                lines: self.lines.clone(),
            },
        }
    }
}

/// What a checkout's pack needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    /// Nothing: it stands as this.
    Leave(PackState),
    /// A fresh copy of the tree, then the installer.
    Copy,
}

/// What a checkout needs for `tree`, when its `.devflow-vendor` is `foreign`
/// or maxd's own note says `ours` is there. A failed install runs again only
/// on `retry`, so an apply tries again and a launch doesn't. One that never
/// finished runs again. Each run gets a fresh copy, since the files in the
/// checkout may have changed since maxd wrote them.
pub fn next(foreign: bool, ours: Option<&Marker>, tree: &TreeHash, retry: bool) -> Next {
    match ours {
        _ if foreign => Next::Leave(PackState::Vendored),
        Some(marker) if marker.tree == *tree => match marker.ran {
            Ran::Installed => Next::Leave(marker.state()),
            Ran::Failed if !retry => Next::Leave(marker.state()),
            Ran::Pending | Ran::Failed => Next::Copy,
        },
        _ => Next::Copy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(tree: &str, ran: Ran) -> Marker {
        Marker {
            tree: TreeHash(tree.into()),
            ran,
            lines: vec!["no uv".into()],
        }
    }

    #[test]
    fn a_copy_without_a_marker_is_someone_elses() {
        let tree = TreeHash("a".into());
        assert_eq!(
            next(true, None, &tree, true),
            Next::Leave(PackState::Vendored)
        );
        assert_eq!(
            next(true, Some(&marker("a", Ran::Installed)), &tree, true),
            Next::Leave(PackState::Vendored)
        );
        assert_eq!(next(false, None, &tree, false), Next::Copy);
    }

    #[test]
    fn each_tree_installs_once_and_a_failure_waits_for_an_apply() {
        let tree = TreeHash("a".into());
        let installed = marker("a", Ran::Installed);
        assert_eq!(
            next(false, Some(&installed), &tree, true),
            Next::Leave(PackState::Installed { tree: tree.clone() })
        );
        let failed = marker("a", Ran::Failed);
        assert!(matches!(
            next(false, Some(&failed), &tree, false),
            Next::Leave(PackState::Failed { .. })
        ));
        assert_eq!(next(false, Some(&failed), &tree, true), Next::Copy);
        assert_eq!(
            next(false, Some(&marker("a", Ran::Pending)), &tree, false),
            Next::Copy
        );
        assert_eq!(
            next(false, Some(&marker("b", Ran::Installed)), &tree, false),
            Next::Copy
        );
    }

    #[test]
    fn the_exclude_lines_are_anchored_at_the_root() {
        let lines = exclude_lines();
        assert!(lines.contains(&"/.devflow-vendor/".to_owned()));
        assert!(lines.contains(&"/CLAUDE.md".to_owned()));
    }
}
