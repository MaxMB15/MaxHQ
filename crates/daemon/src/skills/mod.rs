// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Skills. Every device keeps a [`store::SkillStore`] of the trees its
//! projects use, the holder keeps the [`library::Library`] of sources, and
//! [`deliver`] gets each project's locked skills to its checkouts. Requests
//! nothing here takes yet are refused in the words here.

pub mod apply;
pub mod deliver;
pub mod globals;
pub mod library;
pub mod pack;
pub mod scan;
pub mod seed;
pub mod sets;
pub mod sources;
pub mod store;

use std::sync::OnceLock;

use maxhq_protocol::ids::SkillSourceId;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::skills::{LockedSkill, SkillSource, SourceKind, SourceTrust};

pub use library::{Library, NewSource};

static LIBRARY: OnceLock<Library> = OnceLock::new();

/// Makes `library` the one the app's requests reach. Only the first call
/// counts, since `maxd` opens one library.
pub fn install(library: Library) {
    let _ = LIBRARY.set(library);
}

/// The installed library, for the code that imports and approves skills.
#[allow(dead_code)]
pub fn installed() -> Option<Library> {
    LIBRARY.get().cloned()
}

/// Answers `SkillSourceAdd`.
pub async fn add(
    name: String,
    kind: SourceKind,
    default_ref: Option<String>,
    trust: SourceTrust,
) -> ActionOutcome {
    let new = NewSource {
        name,
        kind,
        default_ref,
        trust,
    };
    match LIBRARY.get() {
        Some(library) => done_or_refused(library.add(new).await.map(drop)),
        None => refused(),
    }
}

/// Answers `SkillSourceFetch`.
pub async fn fetch(source: SkillSourceId) -> ActionOutcome {
    match LIBRARY.get() {
        Some(library) => done_or_refused(library.fetch(source).await.map(drop)),
        None => refused(),
    }
}

/// Answers `SkillSourceRemove`.
pub async fn remove(source: SkillSourceId) -> ActionOutcome {
    match LIBRARY.get() {
        Some(library) => done_or_refused(library.remove(source).await),
        None => refused(),
    }
}

/// Answers `SkillSourcePush`.
pub async fn push(source: SkillSourceId) -> ActionOutcome {
    match LIBRARY.get() {
        Some(library) => done_or_refused(library.push(source).await),
        None => refused(),
    }
}

/// Answers `SkillsTakeLibrary`.
pub async fn take(from: String, sources: Vec<SkillSource>) -> ActionOutcome {
    match LIBRARY.get() {
        Some(library) => done_or_refused(library.take(&from, sources).await),
        None => refused(),
    }
}

/// Answers `SkillsSetAlways`.
pub fn set_always(skills: Vec<LockedSkill>) -> ActionOutcome {
    match LIBRARY.get() {
        Some(library) => done_or_refused(library.set_always(skills)),
        None => refused(),
    }
}

fn done_or_refused(result: Result<(), String>) -> ActionOutcome {
    match result {
        Ok(()) => ActionOutcome::Done,
        Err(reason) => ActionOutcome::Refused { reason },
    }
}

/// The refusal for every skills request while this device keeps none.
pub const NOT_YET: &str = "Skills aren't set up yet.";

/// The answer to an app request about skills while there are none.
pub fn refused() -> ActionOutcome {
    ActionOutcome::Refused {
        reason: NOT_YET.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_says_skills_are_not_set_up() {
        assert_eq!(
            refused(),
            ActionOutcome::Refused {
                reason: "Skills aren't set up yet.".into()
            }
        );
    }
}
