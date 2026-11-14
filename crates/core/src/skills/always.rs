// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The always set: the skills MaxHQ keeps global on every device, which
//! project skills can't be named after. `unslop` comes from the library.
//! `propose-skill-change` is MaxHQ's own, kept here as text and written
//! out when a device needs its folder.

use std::fs;
use std::io;
use std::path::Path;

use maxhq_protocol::skills::TreeHash;

use super::mem::MemFs;
use super::{tree, SKILL_FILE};

/// The skill that tells agents how to propose a skill change.
pub const PROPOSE: &str = "propose-skill-change";

/// The always set's names.
pub const NAMES: [&str; 2] = ["unslop", PROPOSE];

/// `propose-skill-change`'s `SKILL.md`.
pub const PROPOSE_SKILL: &str = "---
name: propose-skill-change
description: Use when you want to change a skill this project uses, or add a new one. The skills here are managed and read only, so write the change as a proposal instead.
---

# Propose a skill change

MaxHQ manages this project's skills. Their folders are read only, and
MaxHQ puts them back before each launch, so an edit there is lost.

To change a skill, or add one:

1. Copy the skill's whole folder to `.maxhq/proposals/<name>/` at the
   checkout's root. For a new skill, make that folder with a `SKILL.md`.
2. Make your changes there. Keep the folder's name and the `name` in
   `SKILL.md` the same.
3. Tell the user what you changed and why. They approve or reject it.

Never edit `.maxhq/skills/`, the links in `.claude/skills/` or
`.agents/skills/`, or any global skill folder.
";

/// The `SKILL.md` of an always skill MaxHQ ships itself.
pub fn builtin(name: &str) -> Option<&'static str> {
    (name == PROPOSE).then_some(PROPOSE_SKILL)
}

/// The tree of an always skill MaxHQ ships itself, worked out without
/// touching the disk.
pub fn builtin_tree(name: &str) -> Option<TreeHash> {
    let text = builtin(name)?;
    let dir = Path::new("/").join(name);
    let mut fs = MemFs::new();
    fs.file(dir.join(SKILL_FILE), text);
    tree::of(&fs, &dir).ok()
}

/// Writes a shipped always skill's folder at `dir`, which mustn't exist
/// yet.
pub fn write_builtin(name: &str, dir: &Path) -> io::Result<()> {
    let text = builtin(name)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{name} isn't shipped")))?;
    fs::create_dir(dir)?;
    fs::write(dir.join(SKILL_FILE), text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::disk::DiskFs;
    use crate::skills::frontmatter;
    use crate::test_dir::TempDir;

    #[test]
    fn the_proposal_skill_reads_as_a_skill() {
        let doc = frontmatter::parse(PROPOSE_SKILL).unwrap();
        assert_eq!(doc.name(), Some(PROPOSE));
        assert!(doc.description().is_some_and(|d| d.len() <= 1024));
        assert!(PROPOSE_SKILL.contains(crate::skills::PROPOSALS_DIR));
        assert_eq!(builtin("unslop"), None);
        assert_eq!(builtin_tree("unslop"), None);
    }

    #[test]
    fn the_written_folder_has_the_shipped_tree() {
        let dir = TempDir::new("always");
        let skill = dir.0.join(PROPOSE);
        write_builtin(PROPOSE, &skill).unwrap();
        assert_eq!(
            tree::of(&DiskFs, &skill).ok(),
            builtin_tree(PROPOSE),
            "the disk and the text agree"
        );
        assert!(write_builtin("unslop", &dir.0.join("unslop")).is_err());
    }
}
