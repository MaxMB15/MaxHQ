// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Folders on a device, since [`crate::message::FOLDERS_SINCE`], for the
//! path fields' suggestions and the folder browser. A client lists one
//! folder at a time with [`crate::ClientMessage::ListFolder`] and makes one
//! with [`crate::ClientMessage::MakeFolder`]. Each entry says whether it's a
//! folder, and a folder that's the top of a git checkout says its branch
//! and project, read from `.git` without running git.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::ProjectId;

/// What [`crate::ClientMessage::ListFolder`] found. A folder that isn't
/// there is an answer too, with no entries, so a field can suggest a parent
/// folder that doesn't exist yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct FolderListing {
    /// The folder's absolute path, with `~` expanded and its links
    /// resolved when it's there.
    pub path: String,
    /// The device's home folder, which a typed `~` stands for.
    pub home: String,
    /// Whether the folder is there.
    pub exists: bool,
    /// What it holds, folders first, each group by name ignoring case.
    pub entries: Vec<FolderEntry>,
    /// Whether it holds more than the entries say, or took too long to
    /// read to the end.
    pub truncated: bool,
}

/// One entry of a [`FolderListing`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct FolderEntry {
    /// Its name in the folder.
    pub name: String,
    /// Whether it's a folder, or a link to one.
    pub folder: bool,
    /// The checkout, when it's the top folder of one.
    pub checkout: Option<FolderCheckout>,
}

/// A git checkout a [`FolderEntry`] is the top of.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct FolderCheckout {
    /// The branch it has checked out, or none on a detached HEAD.
    pub branch: Option<String>,
    /// Its project, as the device names it, the same on every device for
    /// checkouts of one remote.
    pub project: ProjectId,
}

/// A folder's listing, or why the app has none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FolderList {
    /// What the folder holds.
    Listed {
        /// The listing.
        listing: FolderListing,
    },
    /// The device's `maxd` is too old to list folders.
    TooOld,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ids::{DeviceId, IdempotencyKey};
    use crate::message::{ActionOutcome, FOLDERS_SINCE};
    use crate::ClientMessage;

    const KEY: &str = "0199a7c2-0000-7000-8000-00000000000a";
    const DEVICE: &str = "0199a7c2-0000-7000-8000-000000000007";
    const PROJECT: &str = "0199a7c2-0000-7000-8000-0000000000b1";

    #[test]
    fn folder_requests_read_back_and_wait_for_their_version() {
        let key: IdempotencyKey = KEY.parse().unwrap();
        let device: DeviceId = DEVICE.parse().unwrap();
        let messages = [
            (
                ClientMessage::ListFolder {
                    key,
                    device,
                    path: "~/code".into(),
                    files: false,
                    hidden: true,
                },
                json!({
                    "type": "list_folder",
                    "key": KEY,
                    "device": DEVICE,
                    "path": "~/code",
                    "files": false,
                    "hidden": true,
                }),
            ),
            (
                ClientMessage::MakeFolder {
                    key,
                    device,
                    path: "/home/dev/code/new".into(),
                },
                json!({
                    "type": "make_folder",
                    "key": KEY,
                    "device": DEVICE,
                    "path": "/home/dev/code/new",
                }),
            ),
        ];
        assert_eq!(FOLDERS_SINCE, 46);
        for (message, wire) in messages {
            assert_eq!(serde_json::to_value(&message).unwrap(), wire);
            assert_eq!(
                serde_json::from_value::<ClientMessage>(wire).unwrap(),
                message
            );
            assert_eq!(message.since(), FOLDERS_SINCE);
            assert!(!message.reaches(Some(45)), "an older maxd never gets it");
            assert!(message.reaches(Some(46)));
            assert_eq!(message.action_key(), Some(key));
        }
    }

    #[test]
    fn a_listing_reads_back() {
        let outcome = ActionOutcome::FolderListed {
            listing: FolderListing {
                path: "/home/dev/code".into(),
                home: "/home/dev".into(),
                exists: true,
                entries: vec![
                    FolderEntry {
                        name: "example-app".into(),
                        folder: true,
                        checkout: Some(FolderCheckout {
                            branch: Some("main".into()),
                            project: PROJECT.parse().unwrap(),
                        }),
                    },
                    FolderEntry {
                        name: "notes.md".into(),
                        folder: false,
                        checkout: None,
                    },
                ],
                truncated: false,
            },
        };
        let wire = json!({
            "status": "folder_listed",
            "listing": {
                "path": "/home/dev/code",
                "home": "/home/dev",
                "exists": true,
                "entries": [
                    {
                        "name": "example-app",
                        "folder": true,
                        "checkout": { "branch": "main", "project": PROJECT },
                    },
                    { "name": "notes.md", "folder": false, "checkout": null },
                ],
                "truncated": false,
            },
        });
        assert_eq!(serde_json::to_value(&outcome).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<ActionOutcome>(wire).unwrap(),
            outcome
        );
        assert_eq!(
            serde_json::to_value(FolderList::TooOld).unwrap(),
            json!({ "status": "too_old" })
        );
    }
}
