// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A project's [`SkillLock`]: checking it, changing it, naming the set it
//! makes, the trees it needs, its mirror in the repo, and the updates its
//! unpinned skills are offered.

use std::collections::BTreeSet;

use maxhq_protocol::skills::{LockedSkill, SkillInfo, SkillLibrary, SkillLock, TreeHash};
use thiserror::Error;

use super::frontmatter::{check_name, NameProblem};
use super::sha256_hex;

/// Why a lock can't be kept.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum LockError {
    /// A name that breaks the spec.
    #[error("{name} {problem}.")]
    BadName {
        /// The name.
        name: String,
        /// How it breaks the spec.
        problem: NameProblem,
    },
    /// The same name twice.
    #[error("{0} is in the set twice.")]
    Twice(String),
    /// The name of a skill every agent gets globally.
    #[error("{0} is one of the skills every agent gets, so a project can't add its own.")]
    Always(String),
}

/// Checks that every name in `lock` is a spec name, is there once, and
/// isn't in the always set.
pub fn check(lock: &SkillLock, always: &[String]) -> Result<(), LockError> {
    let mut seen = BTreeSet::new();
    for skill in &lock.skills {
        let name = &skill.name;
        check_name(name).map_err(|problem| LockError::BadName {
            name: name.clone(),
            problem,
        })?;
        if !seen.insert(name.as_str()) {
            return Err(LockError::Twice(name.clone()));
        }
        if always.contains(name) {
            return Err(LockError::Always(name.clone()));
        }
    }
    Ok(())
}

/// Puts `skill` in `lock`, replacing the one of the same name, and keeps
/// the skills sorted by name.
pub fn put(lock: &mut SkillLock, skill: LockedSkill) {
    match lock.skills.binary_search_by(|s| s.name.cmp(&skill.name)) {
        Ok(at) => lock.skills[at] = skill,
        Err(at) => lock.skills.insert(at, skill),
    }
}

/// Takes the skill `name` out of `lock`, saying whether it was there.
pub fn remove(lock: &mut SkillLock, name: &str) -> bool {
    let before = lock.skills.len();
    lock.skills.retain(|s| s.name != name);
    lock.skills.len() != before
}

/// Names the set of skills `lock` makes: hex SHA-256 over each name and
/// tree, sorted by name, then the devflow pack's tree when it has one. Two
/// locks with the same skills and pack at the same trees share it,
/// whatever their sources, commits or pins.
pub fn set_hash(lock: &SkillLock) -> String {
    let mut pairs: Vec<(&str, &str)> = lock
        .skills
        .iter()
        .map(|s| (s.name.as_str(), s.tree.0.as_str()))
        .collect();
    pairs.sort_unstable();
    let mut records = String::new();
    for (name, tree) in pairs {
        records.push_str(name);
        records.push('\0');
        records.push_str(tree);
        records.push('\n');
    }
    if let Some(pack) = &lock.pack {
        // No skill name holds a NUL, so this can't read as a skill.
        records.push_str("\0pack\0");
        records.push_str(&pack.tree.0);
        records.push('\n');
    }
    sha256_hex(records.as_bytes())
}

/// The trees `lock` uses that `has` says the store lacks, each once, its
/// devflow pack's among them.
pub fn missing(lock: &SkillLock, has: impl Fn(&TreeHash) -> bool) -> Vec<TreeHash> {
    let skills = lock.skills.iter().map(|s| &s.tree);
    let trees: BTreeSet<&TreeHash> = skills.chain(lock.pack.iter().map(|p| &p.tree)).collect();
    trees.into_iter().filter(|t| !has(t)).cloned().collect()
}

/// What `maxd` writes to the repo's lock mirror: the lock as pretty JSON,
/// sorted by name, ending in a newline.
pub fn mirror(lock: &SkillLock) -> String {
    let mut sorted = lock.clone();
    sorted.skills.sort_by(|a, b| a.name.cmp(&b.name));
    let mut text = serde_json::to_string_pretty(&sorted).unwrap_or_default();
    text.push('\n');
    text
}

/// A newer version of a skill in a lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update<'a> {
    /// The skill as the lock has it.
    pub locked: &'a LockedSkill,
    /// The skill as its source has it now.
    pub info: &'a SkillInfo,
    /// The source's commit now.
    pub commit: Option<&'a str>,
}

impl Update<'_> {
    /// The lock entry the update makes, keeping the name and pin.
    pub fn applied(&self) -> LockedSkill {
        LockedSkill {
            tree: self.info.tree.clone(),
            commit: self.commit.map(str::to_owned),
            ..self.locked.clone()
        }
    }
}

/// The updates `library` offers `lock`: each unpinned skill whose source
/// now has a different tree at the same path.
pub fn updates<'a>(lock: &'a SkillLock, library: &'a SkillLibrary) -> Vec<Update<'a>> {
    lock.skills
        .iter()
        .filter(|locked| !locked.pinned)
        .filter_map(|locked| {
            let source = library.sources.iter().find(|s| s.id == locked.source)?;
            let info = source.skills.iter().find(|i| i.path == locked.path)?;
            (info.tree != locked.tree).then_some(Update {
                locked,
                info,
                commit: source.commit.as_deref(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::ids::{DeviceId, SkillSourceId};
    use maxhq_protocol::skills::{SkillFlags, SkillPack, SkillSource, SourceKind, SourceTrust};

    use super::*;

    fn source() -> SkillSourceId {
        SkillSourceId::derive("example/agent-skills")
    }

    fn locked(name: &str, tree: &str, pinned: bool) -> LockedSkill {
        LockedSkill {
            name: name.into(),
            source: source(),
            path: format!("skills/{name}"),
            commit: Some("old".into()),
            tree: TreeHash(tree.into()),
            pinned,
        }
    }

    fn lock(skills: Vec<LockedSkill>) -> SkillLock {
        SkillLock {
            skills,
            committed: false,
            pack: None,
        }
    }

    fn pack(tree: &str) -> SkillPack {
        SkillPack {
            source: source(),
            commit: "abc".into(),
            tree: TreeHash(tree.into()),
        }
    }

    #[test]
    fn a_lock_refuses_bad_twice_and_always_names() {
        let always = vec!["unslop".to_owned()];
        assert_eq!(
            check(&lock(vec![locked("git-flow", "a", false)]), &always),
            Ok(())
        );
        assert!(matches!(
            check(&lock(vec![locked("Git Flow", "a", false)]), &always),
            Err(LockError::BadName { .. })
        ));
        assert_eq!(
            check(
                &lock(vec![locked("a", "1", false), locked("a", "2", false)]),
                &always
            ),
            Err(LockError::Twice("a".into()))
        );
        assert_eq!(
            check(&lock(vec![locked("unslop", "1", false)]), &always),
            Err(LockError::Always("unslop".into()))
        );
    }

    #[test]
    fn put_replaces_by_name_and_keeps_order() {
        let mut set = SkillLock::default();
        put(&mut set, locked("b", "1", false));
        put(&mut set, locked("a", "1", false));
        put(&mut set, locked("b", "2", true));
        let names: Vec<_> = set
            .skills
            .iter()
            .map(|s| (s.name.as_str(), s.pinned))
            .collect();
        assert_eq!(names, [("a", false), ("b", true)]);
        assert!(remove(&mut set, "a"));
        assert!(!remove(&mut set, "a"));
    }

    #[test]
    fn the_set_hash_ignores_order_sources_and_pins() {
        let one = lock(vec![locked("a", "1", false), locked("b", "2", false)]);
        let mut two = lock(vec![locked("b", "2", true), locked("a", "1", false)]);
        two.skills[0].commit = None;
        assert_eq!(set_hash(&one), set_hash(&two));
        assert_ne!(
            set_hash(&one),
            set_hash(&lock(vec![locked("a", "1", false)]))
        );
    }

    #[test]
    fn the_set_hash_and_missing_trees_follow_the_pack() {
        let plain = lock(vec![locked("a", "1", false)]);
        let mut packed = plain.clone();
        packed.pack = Some(pack("9"));
        assert_ne!(set_hash(&plain), set_hash(&packed));
        let mut moved = packed.clone();
        moved.pack = Some(pack("8"));
        assert_ne!(set_hash(&packed), set_hash(&moved));
        assert_eq!(missing(&packed, |t| t.0 == "1"), vec![TreeHash("9".into())]);
        assert!(!mirror(&plain).contains("pack"));
    }

    #[test]
    fn missing_names_each_tree_the_store_lacks_once() {
        let set = lock(vec![
            locked("a", "1", false),
            locked("b", "1", false),
            locked("c", "2", false),
        ]);
        assert_eq!(missing(&set, |t| t.0 == "2"), vec![TreeHash("1".into())]);
    }

    #[test]
    fn the_mirror_is_sorted_json() {
        let text = mirror(&lock(vec![
            locked("b", "1", false),
            locked("a", "1", false),
        ]));
        let back: SkillLock = serde_json::from_str(&text).unwrap();
        assert_eq!(back.skills[0].name, "a");
        assert!(text.ends_with("}\n"));
    }

    #[test]
    fn only_unpinned_skills_with_a_new_tree_are_offered_updates() {
        let info = |name: &str, tree: &str| SkillInfo {
            name: name.into(),
            description: "d".into(),
            tree: TreeHash(tree.into()),
            path: format!("skills/{name}"),
            source: source(),
            flags: SkillFlags::default(),
            listing_chars: 10,
        };
        let library = SkillLibrary {
            device: DeviceId::derive("mac"),
            sources: vec![SkillSource {
                id: source(),
                name: "agent-skills".into(),
                kind: SourceKind::Git {
                    url: "https://github.com/owner/repo".into(),
                },
                default_ref: None,
                trust: SourceTrust::Own,
                commit: Some("new".into()),
                fetched_at: None,
                problem: None,
                skills: vec![info("a", "2"), info("b", "2"), info("c", "1")],
                pack: None,
                unpushed: false,
            }],
            always: Vec::new(),
            always_skills: Vec::new(),
        };
        let set = lock(vec![
            locked("a", "1", false),
            locked("b", "1", true),
            locked("c", "1", false),
            locked("gone", "1", false),
        ]);
        let found = updates(&set, &library);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].locked.name, "a");
        let applied = found[0].applied();
        assert_eq!(applied.tree, TreeHash("2".into()));
        assert_eq!(applied.commit.as_deref(), Some("new"));
        assert!(!applied.pinned);
    }
}
