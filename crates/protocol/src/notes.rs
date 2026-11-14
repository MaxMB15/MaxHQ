// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Project notes. A note belongs to a project and lives outside its
//! repository. Every device that has the project keeps every version of
//! its notes, and devices swap the versions the other lacks, so they all
//! end at the same head. Each save is a [`NoteVersion`] named by its
//! [`VersionHash`], and two heads merge into a version of their own,
//! which every device makes the same way. Each device publishes what it
//! holds as a [`ProjectNotes`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

pub use crate::ids::NoteId;
use crate::ids::{AgentId, DeviceId, ProjectId};
use crate::time::Timestamp;

/// The most characters a note's title may have.
pub const MAX_NOTE_TITLE: usize = 200;

/// The most bytes a note's body may have, 1 MiB.
pub const MAX_NOTE_BODY: usize = 1024 * 1024;

/// The most versions a [`crate::ClientMessage::NoteHistory`] answers with.
pub const MAX_NOTE_HISTORY: usize = 200;

/// A note version's hash: lowercase hex SHA-256 over a canonical JSON of
/// its note, sorted parents, title, body and whether it's deleted. When
/// and by whom stay out, so two devices that make the same merge make the
/// same version.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type, JsonSchema,
)]
#[serde(transparent)]
pub struct VersionHash(pub String);

/// Who saved a version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct NoteAuthor {
    /// The device it was saved on.
    pub device: DeviceId,
    /// The agent that saved it, or none when you did.
    pub agent: Option<AgentId>,
}

/// One version of a note, as every device keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct NoteVersion {
    /// Its hash, which names it.
    pub hash: VersionHash,
    /// The project the note is in.
    pub project: ProjectId,
    /// The note.
    pub note: NoteId,
    /// The versions it follows, sorted. None for a note's first version,
    /// two for a merge.
    pub parents: Vec<VersionHash>,
    /// The title, at most [`MAX_NOTE_TITLE`] characters.
    pub title: String,
    /// The body, as Markdown, at most [`MAX_NOTE_BODY`] bytes.
    pub body: String,
    /// Whether it's deleted. Restoring a note is a new version that isn't.
    pub deleted: bool,
    /// When it was saved. A merge has the later of its two heads' times.
    pub at: Timestamp,
    /// Who saved it. None on a merge, so every device makes the same
    /// record.
    pub by: Option<NoteAuthor>,
}

/// One note as a device holds it, for the sidebar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct NoteSummary {
    /// The note.
    pub note: NoteId,
    /// Its title at its head.
    pub title: String,
    /// Its head, the one version left after merging.
    pub head: VersionHash,
    /// When the head was saved.
    pub at: Timestamp,
    /// Who saved the head.
    pub by: Option<NoteAuthor>,
    /// The device its first version was saved on, which tells two notes
    /// with the same title apart.
    pub created_on: DeviceId,
    /// Whether the head is deleted.
    pub deleted: bool,
    /// The head's body, in bytes.
    pub size: u32,
}

/// One version in a note's history, without its body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct NoteVersionInfo {
    /// Its hash.
    pub hash: VersionHash,
    /// The versions it follows, sorted.
    pub parents: Vec<VersionHash>,
    /// Its title.
    pub title: String,
    /// Whether it's deleted.
    pub deleted: bool,
    /// When it was saved.
    pub at: Timestamp,
    /// Who saved it, or none for a merge.
    pub by: Option<NoteAuthor>,
    /// Its body, in bytes.
    pub size: u32,
}

/// The notes one device holds for one project, tombstones included, so a
/// delete reaches every device. A device publishes none for a project
/// with no notes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ProjectNotes {
    /// The device that holds them.
    pub device: DeviceId,
    /// The project.
    pub project: ProjectId,
    /// Each note at its head.
    pub notes: Vec<NoteSummary>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_version_reads_back_with_its_fields_in_snake_case() {
        let device = DeviceId::derive("devbox");
        let version = NoteVersion {
            hash: VersionHash("ab".repeat(32)),
            project: ProjectId::derive("p"),
            note: NoteId::derive("n"),
            parents: vec![VersionHash("cd".repeat(32))],
            title: "Release checklist".into(),
            body: "- [x] Tag\n".into(),
            deleted: false,
            at: Timestamp(1_761_000_000_000),
            by: Some(NoteAuthor {
                device,
                agent: None,
            }),
        };
        let value = serde_json::to_value(&version).unwrap();
        assert_eq!(value["hash"], json!("ab".repeat(32)));
        assert_eq!(value["by"], json!({ "device": device, "agent": null }));
        let back: NoteVersion = serde_json::from_value(value).unwrap();
        assert_eq!(back, version);
    }
}
