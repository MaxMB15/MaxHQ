// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Filling the library from the skills a device already has, which the
//! tidy does once you agree to it. The tidy's plan walks the global
//! folders. This reads which repo each skill came from: a skill
//! `~/.agents/.skill-lock.json` tracks comes from its repo, which joins as
//! a third-party source, and the rest go into the personal source.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

/// Where the `skills` CLI notes the repo each skill it installed came from.
pub const SKILL_LOCK: &str = ".agents/.skill-lock.json";

#[derive(Deserialize)]
struct SkillLock {
    #[serde(default)]
    skills: BTreeMap<String, Tracked>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tracked {
    source_url: Option<String>,
}

/// The repo each skill the skill lock under `home` tracks came from, by
/// name. A missing or unreadable lock tracks none.
pub fn tracked(home: &Path) -> BTreeMap<String, String> {
    fs::read_to_string(home.join(SKILL_LOCK))
        .ok()
        .and_then(|text| serde_json::from_str::<SkillLock>(&text).ok())
        .map(|lock| {
            lock.skills
                .into_iter()
                .filter_map(|(name, skill)| Some((name, skill.source_url?)))
                .filter(|(_, url)| !url.trim().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn the_skill_lock_names_each_skills_repo() {
        let dir = TempDir::new();
        assert!(tracked(dir.path()).is_empty());
        fs::create_dir_all(dir.path().join(".agents")).unwrap();
        fs::write(
            dir.path().join(SKILL_LOCK),
            r#"{"version":3,"skills":{
                "pdf":{"source":"owner/skills","sourceUrl":"https://github.com/owner/skills.git"},
                "notes":{"source":"local"}
            }}"#,
        )
        .unwrap();
        assert_eq!(
            tracked(dir.path()),
            BTreeMap::from([(
                "pdf".to_owned(),
                "https://github.com/owner/skills.git".to_owned()
            )])
        );
        fs::write(dir.path().join(SKILL_LOCK), "not json").unwrap();
        assert!(tracked(dir.path()).is_empty());
    }
}
