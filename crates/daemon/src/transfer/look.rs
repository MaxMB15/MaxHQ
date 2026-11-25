// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Checks the file an agent opens with MaxHQ's open tool, before its card
//! goes on the timeline. The file must be there, be a regular file of a
//! kind the viewer opens, and be no larger than the viewer takes. A path
//! outside the agent's folders is checked only by its name, until the user
//! picks Show anyway. Nothing here reads the file's bytes. A file in a container is checked from the
//! manifest its guest sends for a download, which is then cancelled.

use std::path::{Component, Path, PathBuf};

use maxhq_client::media;
use maxhq_protocol::guest::HostToGuest;
use maxhq_protocol::ids::{TransferId, WorkspaceId};
use maxhq_protocol::message::ErrorCode;
use maxhq_transfer::manifest::{self, Kind};
use maxhq_transfer::Failure;
use tracing::debug;

use super::disk;
use super::push;
use super::registry::Peer;
use super::{guest_path, in_guest_folders, Event, FromGuest, Task, Transfers, STALL};
use crate::drive::{Place, PlaceHost};
use crate::session::holders::Looked;

impl Transfers {
    /// Checks the file at `path` for the open tool, on this device or in
    /// the agent's container. An error is what the agent reads.
    pub(super) async fn look_at(&self, place: &Place, path: &str) -> Result<Looked, String> {
        match place.host {
            PlaceHost::Local => {
                let roots = vec![place.cwd.clone(), place.attachments.clone()];
                let path = path.to_owned();
                tokio::task::spawn_blocking(move || look_local(&roots, &path))
                    .await
                    .unwrap_or_else(|_| Err("MaxHQ couldn't check the file.".to_owned()))
            }
            PlaceHost::Guest { workspace } => self.look_guest(place, workspace, path).await,
        }
    }

    /// Checks a file in the agent's container from the manifest its guest
    /// sends, and cancels the download before any bytes move.
    async fn look_guest(
        &self,
        place: &Place,
        workspace: WorkspaceId,
        path: &str,
    ) -> Result<Looked, String> {
        let said = |failure: Failure| format!("{path}: {}.", failure.message);
        let inside = guest_path(place, path).map_err(said)?;
        if !in_guest_folders(place, &inside) {
            return Err(format!(
                "{path} is outside your folders. MaxHQ shows a file from outside them \
                 only for agents on the device itself, so copy it into your folder first."
            ));
        }
        kind_shown(path)?;
        let transfer = TransferId::generate();
        let link = self
            .shared
            .link(workspace)
            .ok_or_else(|| said(push::unreachable()))?;
        let inbox = self
            .shared
            .open
            .open(transfer, vec![Peer::Guest(workspace)])
            .ok_or_else(|| "MaxHQ couldn't check the file.".to_owned())?;
        let mut task = Task {
            shared: self.shared.clone(),
            inbox,
            route: None,
            key: None,
            transfer,
            stash: None,
        };
        let begin = HostToGuest::FetchBegin {
            transfer,
            path: inside.clone(),
        };
        link.message(&begin)
            .await
            .map_err(|_| said(push::unreachable()))?;
        let heard = loop {
            match task.next_within(STALL).await {
                Some(Event::Guest(FromGuest::Manifest(entries))) => break Ok(entries),
                Some(Event::Guest(FromGuest::Failed(failure))) => break Err(failure),
                Some(Event::GuestDown) => break Err(push::unreachable()),
                None => {
                    break Err(Failure::new(
                        ErrorCode::Unreachable,
                        None,
                        "the container didn't answer",
                    ))
                }
                Some(_) => debug!("dropped an event a look doesn't take"),
            }
        };
        link.cancel(transfer).await;
        let entries = match heard {
            Ok(entries) => entries,
            Err(failure) => return Err(said(failure)),
        };
        let checked = manifest::check(&entries).map_err(said)?;
        match manifest::tops(&checked).as_slice() {
            [top] => match top.kind {
                Kind::File { .. } => {
                    let size = entries.first().map_or(0, |entry| entry.size);
                    fits(path, size)?;
                    Ok(Looked {
                        path: inside,
                        outside: false,
                    })
                }
                Kind::Dir => Err(folder(path)),
                Kind::Symlink { .. } => Err(link_refused(path)),
            },
            _ => Err(folder(path)),
        }
    }
}

/// Checks the file at `path` on this device. A relative path is from the
/// first of `roots`. An absolute one outside all of them is allowed, since
/// the user asked for it, but is flagged so the app asks before showing it.
pub fn look_local(roots: &[PathBuf], path: &str) -> Result<Looked, String> {
    // A folder on the way that isn't there.
    let gone = || {
        let named = match roots.first() {
            Some(first) if Path::new(path).is_relative() => first.join(path),
            _ => PathBuf::from(path),
        };
        std::fs::symlink_metadata(named)
            .is_err_and(|err| err.kind() == std::io::ErrorKind::NotFound)
    };
    match disk::resolve(roots, path) {
        Ok(found) => {
            check_file(path, &found)?;
            Ok(Looked {
                path: found.display().to_string(),
                outside: false,
            })
        }
        // A path outside isn't looked at, so the agent hears the same
        // whatever is there and learns nothing of the disk past its
        // folders. Show anyway checks the file once the user asks.
        Err(failure) if failure.code == ErrorCode::Outside => {
            if Path::new(path)
                .components()
                .any(|part| part == Component::ParentDir)
            {
                return Err(format!("{path}: the path has a `..` part."));
            }
            kind_shown(path)?;
            Ok(Looked {
                path: path.to_owned(),
                outside: true,
            })
        }
        Err(failure) if failure.message == "nothing is at that path" => Err(missing(path)),
        Err(failure) if failure.code == ErrorCode::Internal && gone() => Err(missing(path)),
        Err(failure) => Err(format!("{path}: {}.", failure.message)),
    }
}

/// Refuses what isn't a regular file the viewer opens and takes.
fn check_file(path: &str, found: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(found).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => missing(path),
        _ => format!("{path}: {err}."),
    })?;
    if meta.is_dir() {
        return Err(folder(path));
    }
    if meta.file_type().is_symlink() {
        return Err(link_refused(path));
    }
    if !meta.is_file() {
        return Err(format!(
            "{path} isn't a regular file, so MaxHQ can't show it."
        ));
    }
    kind_shown(path)?;
    if found
        .to_str()
        .is_none_or(|found| media::extension(found).is_none())
    {
        return Err(unsupported(path));
    }
    fits(path, meta.len())
}

/// Refuses a name whose kind the viewer doesn't open.
fn kind_shown(path: &str) -> Result<(), String> {
    media::extension(path)
        .map(|_| ())
        .ok_or_else(|| unsupported(path))
}

/// Refuses a file larger than the viewer takes.
fn fits(path: &str, size: u64) -> Result<(), String> {
    if size > media::MOST {
        return Err(format!(
            "{path} is larger than the 1 GB MaxHQ's viewer opens."
        ));
    }
    Ok(())
}

fn missing(path: &str) -> String {
    format!("There's no file at {path}. Check the path and try again.")
}

fn folder(path: &str) -> String {
    format!("{path} is a folder. Open one of the files in it instead.")
}

fn link_refused(path: &str) -> String {
    format!("{path} is a link, which MaxHQ won't follow. Open the file it points at instead.")
}

fn unsupported(path: &str) -> String {
    format!(
        "MaxHQ's viewer doesn't open that kind of file, so {path} can't be shown. \
         Tell the user where it is instead."
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use maxhq_core::fs_ext::symlink;

    use super::*;
    use crate::testing::TempDir;

    /// An agent's folder and attachments folder, with a file of each kind
    /// the checks tell apart, and a file outside both.
    fn folders() -> (TempDir, Vec<PathBuf>, PathBuf) {
        let dir = TempDir::new();
        let cwd = dir.path().join("work");
        let attachments = dir.path().join("attachments");
        let elsewhere = dir.path().join("elsewhere");
        for folder in [&cwd, &attachments, &elsewhere] {
            fs::create_dir_all(folder).unwrap();
        }
        fs::create_dir_all(cwd.join("docs/pictures.png")).unwrap();
        fs::write(cwd.join("docs/plan.md"), "# Plan").unwrap();
        fs::write(cwd.join("main.exe"), "binary").unwrap();
        fs::write(elsewhere.join("chart.png"), "png").unwrap();
        symlink(cwd.join("docs/plan.md"), cwd.join("linked.md")).unwrap();
        let outside = maxhq_core::fs_ext::canonicalize(&elsewhere).unwrap();
        (dir, vec![cwd, attachments], outside)
    }

    #[test]
    fn a_relative_path_inside_resolves_to_an_absolute_one() {
        let (_dir, roots, _) = folders();
        let looked = look_local(&roots, "docs/plan.md").unwrap();
        let real = roots[0].canonicalize().unwrap().join("docs/plan.md");
        assert_eq!(
            looked,
            Looked {
                path: real.display().to_string(),
                outside: false,
            }
        );
        let absolute = roots[0].join("docs/plan.md");
        let again = look_local(&roots, absolute.to_str().unwrap()).unwrap();
        assert!(!again.outside);
    }

    #[test]
    fn an_absolute_path_outside_the_folders_is_flagged_whatever_is_there() {
        let (_dir, roots, outside) = folders();
        fs::create_dir_all(outside.join("album.png")).unwrap();
        for name in ["chart.png", "gone.png", "album.png"] {
            let path = outside.join(name);
            let looked = look_local(&roots, path.to_str().unwrap()).unwrap();
            assert_eq!(
                looked,
                Looked {
                    path: path.display().to_string(),
                    outside: true,
                }
            );
        }
        let refused = |path: PathBuf| look_local(&roots, path.to_str().unwrap()).unwrap_err();
        assert!(refused(outside.join("main.exe")).contains("doesn't open that kind of file"));
        assert!(refused(outside.join("../elsewhere/chart.png")).contains(".."));
    }

    #[test]
    fn a_missing_file_a_folder_a_link_and_an_unshown_kind_are_refused() {
        let (_dir, roots, _) = folders();
        let refused = |path: &str| look_local(&roots, path).expect_err(path);
        assert!(refused("docs/gone.md").starts_with("There's no file at docs/gone.md"));
        assert!(refused("nowhere/gone.md").starts_with("There's no file"));
        assert!(refused("docs/pictures.png").contains("is a folder"));
        assert!(refused("docs").contains("is a folder"));
        assert!(refused("main.exe").contains("doesn't open that kind of file"));
        assert!(refused("linked.md").contains("is a link"));
        assert!(refused("../elsewhere/chart.png").contains(".."));
        assert!(refused("../elsewhere/gone.png").contains(".."));
    }
}
