// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A device's notes over an in-memory database: saves, merges, puts, the
//! client requests and what the hub shows.

use std::collections::HashMap;

use maxhq_core::notes::sealed;
use maxhq_protocol::ids::{BootId, WorkspaceId};
use maxhq_protocol::model::{Device, Workspace, WorkspaceLocation};
use maxhq_protocol::notes::{MAX_NOTE_BODY, MAX_NOTE_TITLE};
use maxhq_protocol::state::EntityKey;

use super::*;

/// A device's hub, with nothing in it but the device.
pub(crate) fn hub_of(name: &str) -> Hub {
    Hub::new(
        Device {
            id: DeviceId::derive(name),
            name: name.into(),
            os: "linux".into(),
            boot_id: BootId("boot".into()),
            maxd_version: "0.1.0".into(),
            winding_down_until: None,
            workspace_host: None,
            build: None,
            target: None,
            wsl: None,
        },
        HashMap::new(),
    )
}

/// Gives `hub`'s device a workspace in `project`.
pub(crate) fn join(hub: &Hub, project: ProjectId) {
    hub.apply([Change::Upsert {
        value: Entity::Workspace(Workspace {
            id: WorkspaceId::generate(),
            project: Some(project),
            device: hub.device().id,
            location: WorkspaceLocation::Folder {
                path: "/home/dev/project".into(),
            },
            managed: None,
            clone: None,
        }),
    }]);
}

/// A save from the user on this device.
pub(crate) fn save(
    project: ProjectId,
    note: NoteId,
    base: Option<&NoteVersion>,
    body: &str,
) -> Save {
    Save {
        project,
        note,
        base: base.map(|version| version.hash.clone()),
        title: "Plan".into(),
        body: body.into(),
        deleted: false,
        at: Timestamp::now(),
        by: None,
    }
}

fn notes() -> (Notes, Hub, Db) {
    let hub = hub_of("devbox");
    let db = Db::open_in_memory().unwrap();
    (Notes::new(db.clone(), hub.clone()), hub, db)
}

fn project() -> ProjectId {
    ProjectId::derive("project")
}

/// What the hub shows of a project's notes.
fn shown(hub: &Hub, project: ProjectId) -> Vec<NoteSummary> {
    match hub.get(&EntityKey::ProjectNotes {
        device: hub.device().id,
        project,
    }) {
        Some(Entity::ProjectNotes(notes)) => notes.notes,
        _ => Vec::new(),
    }
}

#[tokio::test]
async fn a_save_makes_a_note_the_hub_shows() {
    let (notes, hub, _) = notes();
    let note = NoteId::generate();
    let (first, merged) = notes
        .save(save(project(), note, None, "one"))
        .await
        .unwrap();
    assert!(!merged);
    assert!(first.parents.is_empty());
    let (second, merged) = notes
        .save(save(project(), note, Some(&first), "two"))
        .await
        .unwrap();
    assert!(!merged);
    assert_eq!(second.parents, vec![first.hash.clone()]);

    let read = notes.read(project(), note, None).await.unwrap();
    assert_eq!(read.body, "two");
    let old = notes
        .read(project(), note, Some(first.hash.clone()))
        .await
        .unwrap();
    assert_eq!(old.body, "one");

    let listed = shown(&hub, project());
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].head, second.hash);
    assert_eq!(listed[0].size, 3);
    assert_eq!(listed[0].created_on, hub.device().id);

    let history = notes.history(project(), note).await.unwrap();
    assert_eq!(history.len(), 2);
    assert!(history.iter().any(|info| info.hash == first.hash));
}

#[tokio::test]
async fn a_save_on_a_stale_base_merges_with_the_head() {
    let (notes, _, _) = notes();
    let note = NoteId::generate();
    let (base, _) = notes
        .save(save(project(), note, None, "one\ntwo\nthree\n"))
        .await
        .unwrap();
    let (theirs, _) = notes
        .save(save(project(), note, Some(&base), "ONE\ntwo\nthree\n"))
        .await
        .unwrap();
    let (head, merged) = notes
        .save(save(project(), note, Some(&base), "one\ntwo\nTHREE\n"))
        .await
        .unwrap();
    assert!(merged);
    assert_eq!(head.body, "ONE\ntwo\nTHREE\n");
    assert_eq!(head.parents.len(), 2);
    assert!(head.parents.contains(&theirs.hash));
    // The version after the stale base is kept too.
    assert_eq!(notes.history(project(), note).await.unwrap().len(), 4);
}

#[tokio::test]
async fn a_new_note_with_a_held_id_merges_against_nothing() {
    let (notes, _, _) = notes();
    let note = NoteId::generate();
    notes
        .save(save(project(), note, None, "mine\n"))
        .await
        .unwrap();
    let (head, merged) = notes
        .save(save(project(), note, None, "theirs\n"))
        .await
        .unwrap();
    assert!(merged);
    assert_eq!(head.parents.len(), 2);
}

#[tokio::test]
async fn a_save_sent_again_changes_nothing() {
    let (notes, _, _) = notes();
    let note = NoteId::generate();
    let again = save(project(), note, None, "one");
    let (first, _) = notes.save(again.clone()).await.unwrap();
    let (second, merged) = notes.save(again).await.unwrap();
    assert_eq!(first, second);
    assert!(!merged);
}

#[tokio::test]
async fn saves_past_the_limits_or_on_an_unknown_base_are_refused() {
    let (notes, _, _) = notes();
    let note = NoteId::generate();
    let mut long = save(project(), note, None, "");
    long.title = "x".repeat(MAX_NOTE_TITLE + 1);
    assert!(notes.save(long).await.is_err());
    let big = save(project(), note, None, &"x".repeat(MAX_NOTE_BODY + 1));
    assert!(notes.save(big).await.is_err());

    let mut stray = save(project(), note, None, "");
    stray.base = Some(VersionHash("ab".repeat(32)));
    assert_eq!(notes.save(stray).await, Err(store::NO_VERSION.into()));
    assert_eq!(
        notes.read(project(), note, None).await,
        Err(store::NO_NOTE.into())
    );
}

#[tokio::test]
async fn a_put_takes_good_versions_and_refuses_a_bad_batch_whole() {
    let (from, _, _) = notes();
    let note = NoteId::generate();
    let (first, _) = from.save(save(project(), note, None, "one")).await.unwrap();
    let (second, _) = from
        .save(save(project(), note, Some(&first), "two"))
        .await
        .unwrap();

    let (to, hub, _) = notes();
    // Parents must come first.
    let backwards = vec![second.clone(), first.clone()];
    assert!(to
        .put(project(), backwards, None)
        .await
        .expect_err("refused")
        .contains("before its parents"));
    // A hash that doesn't match is refused.
    let mut forged = first.clone();
    forged.body = "forged".into();
    assert!(to.put(project(), vec![forged], None).await.is_err());
    // As is one from another project.
    assert!(to
        .put(ProjectId::derive("other"), vec![first.clone()], None)
        .await
        .is_err());
    assert!(to.hashes(project()).await.unwrap().is_empty());

    let both = vec![first.clone(), second.clone()];
    assert_eq!(to.put(project(), both.clone(), None).await, Ok(2));
    assert_eq!(to.put(project(), both, None).await, Ok(0));
    assert_eq!(to.read(project(), note, None).await.unwrap(), second);
    assert_eq!(shown(&hub, project())[0].head, second.hash);
    // The note was made on the other device, by the version's author.
    let mut hashes = vec![first.hash, second.hash];
    hashes.sort();
    assert_eq!(to.hashes(project()).await.unwrap(), hashes);
}

#[tokio::test]
async fn a_put_of_two_heads_merges_them() {
    let (notes, _, _) = notes();
    let note = NoteId::generate();
    let root = sealed(NoteVersion {
        hash: VersionHash(String::new()),
        project: project(),
        note,
        parents: Vec::new(),
        title: "Plan".into(),
        body: "a\nb\nc\n".into(),
        deleted: false,
        at: Timestamp(1),
        by: None,
    });
    let side = |body: &str, at| {
        sealed(NoteVersion {
            parents: vec![root.hash.clone()],
            body: body.into(),
            at: Timestamp(at),
            ..root.clone()
        })
    };
    let left = side("A\nb\nc\n", 2);
    let right = side("a\nb\nC\n", 3);
    assert_eq!(
        notes
            .put(project(), vec![root, left.clone(), right.clone()], None)
            .await,
        Ok(3)
    );
    let head = notes.read(project(), note, None).await.unwrap();
    assert_eq!(head.body, "A\nb\nC\n");
    let mut parents = head.parents.clone();
    parents.sort();
    let mut expected = vec![left.hash, right.hash];
    expected.sort();
    assert_eq!(parents, expected);
}

#[tokio::test]
async fn a_deleted_note_stays_listed_as_a_tombstone() {
    let (notes, hub, _) = notes();
    let note = NoteId::generate();
    let (first, _) = notes
        .save(save(project(), note, None, "one"))
        .await
        .unwrap();
    let mut gone = save(project(), note, Some(&first), "");
    gone.deleted = true;
    notes.save(gone).await.unwrap();
    let listed = shown(&hub, project());
    assert_eq!(listed.len(), 1);
    assert!(listed[0].deleted);
}

#[tokio::test]
async fn a_client_request_answers_with_its_outcome() {
    let (notes, _, _) = notes();
    let note = NoteId::generate();
    let saved = notes
        .answer(ClientMessage::SaveNote {
            key: maxhq_protocol::ids::IdempotencyKey::generate(),
            project: project(),
            note,
            base: None,
            title: "Plan".into(),
            body: "one".into(),
            deleted: false,
        })
        .await;
    let ActionOutcome::NoteSaved { version, merged } = saved else {
        panic!("not saved: {saved:?}");
    };
    assert!(!merged);
    assert_eq!(
        version.by,
        Some(NoteAuthor {
            device: notes.me(),
            agent: None
        })
    );
    let read = notes
        .answer(ClientMessage::ReadNote {
            key: maxhq_protocol::ids::IdempotencyKey::generate(),
            project: project(),
            note,
            version: None,
        })
        .await;
    assert_eq!(read, ActionOutcome::NoteRead { version });
}

#[tokio::test]
async fn a_peer_gets_nothing_for_a_project_this_device_doesnt_keep() {
    let (notes, hub, _) = notes();
    let peer = DeviceId::derive("laptop");
    let (theirs, _, _) = self::notes();
    let note = NoteId::generate();
    let (version, _) = theirs
        .save(save(project(), note, None, "one"))
        .await
        .unwrap();

    let put = PeerRequest::PutNoteVersions {
        project: project(),
        versions: vec![version.clone()],
    };
    assert_eq!(
        notes.serve(peer, put.clone()).await,
        ActionOutcome::Refused {
            reason: NOT_KEPT.into()
        }
    );
    let asked = PeerRequest::NoteHashes { project: project() };
    assert_eq!(
        notes.serve(peer, asked.clone()).await,
        ActionOutcome::NoteHashes { hashes: Vec::new() }
    );

    // A workspace in the project makes it one this device keeps.
    join(&hub, project());
    assert_eq!(
        notes.serve(peer, put).await,
        ActionOutcome::NotesPut { stored: 1 }
    );
    assert_eq!(
        notes.serve(peer, asked).await,
        ActionOutcome::NoteHashes {
            hashes: vec![version.hash]
        }
    );
}

#[tokio::test]
async fn notes_held_already_are_kept_without_a_workspace() {
    let (notes, _, _) = notes();
    let note = NoteId::generate();
    notes
        .save(save(project(), note, None, "one"))
        .await
        .unwrap();
    assert_eq!(notes.keeps(project()).await, Ok(true));
    assert_eq!(notes.keeps(ProjectId::derive("other")).await, Ok(false));
}

#[tokio::test]
async fn notes_held_at_start_are_shown() {
    let (notes, _, db) = notes();
    notes
        .save(save(project(), NoteId::generate(), None, "one"))
        .await
        .unwrap();
    let hub = hub_of("devbox");
    let again = Notes::new(db, hub.clone());
    assert!(shown(&hub, project()).is_empty());
    again.show_all().await;
    assert_eq!(shown(&hub, project()).len(), 1);
}
