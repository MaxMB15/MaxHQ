// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Files between this machine and a device. An upload sends a file or
//! folder you handed over to a device's staging area, a workspace or an
//! agent's folder, and a download fetches one an agent can see into
//! `~/Downloads/MaxHQ`. A picture or video the timeline shows comes the
//! way a download does, into the media cache that [`crate::media`] serves
//! the webview from. A file an agent can see, such as a picture in its
//! timeline, can go to another agent's draft: it comes the way a download
//! does into a copy here, then goes on as a dropped file would. Each
//! transfer reports how far it has got to a report of its own, which hears
//! done or failed last.
//!
//! A project copies to another device the same way: the target clones the
//! project, then the files you chose from the source's ignored ones come
//! into a copy here and go on into the new checkout.
//!
//! A path to upload has to be one you dropped, pasted or picked, as
//! [`Offered`] keeps them, so a script in the page can't send your files
//! anywhere.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use maxhq_client::media::Media;
use maxhq_protocol::ids::{AgentId, DeviceId, IdempotencyKey, TransferId, WorkspaceId};
use maxhq_protocol::message::{ActionOutcome, AttachmentSource, PlaceFailure, UploadTarget};
use maxhq_protocol::model::{CloneState, GuestState, WorkspaceSpec, WorkspaceState};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::ClientMessage;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::devices::{Connector, Devices, OnProgress};
use crate::merge::Merge;
use crate::offered::Offered;
use crate::request::{lookup_failed, ActionReply, CommandError};
use crate::view::View;
use crate::App;

/// How far one transfer has got, as its report hears it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct TransferProgress {
    /// The transfer, which a `cancel_transfer` request takes.
    pub transfer: TransferId,
    /// Bytes on their way so far, across every file in it.
    #[specta(type = specta_typescript::Number)]
    pub sent: u64,
    /// Bytes in the whole transfer, or 0 before it's known.
    #[specta(type = specta_typescript::Number)]
    pub total: u64,
    /// The file on its way now, as a path inside the transfer.
    pub file: String,
    /// What the transfer is doing.
    pub state: TransferState,
}

/// What a transfer is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    /// Bytes are moving.
    Uploading,
    /// The far end has no room yet, so bytes wait for it.
    Waiting,
    /// The link dropped. It picks up again once the link is back.
    Paused,
    /// Every byte arrived.
    Done,
    /// It stopped for good.
    Failed,
}

impl From<maxhq_client::TransferProgress> for TransferProgress {
    fn from(progress: maxhq_client::TransferProgress) -> Self {
        Self {
            transfer: progress.transfer,
            sent: progress.sent,
            total: progress.total,
            file: progress.file,
            state: progress.state.into(),
        }
    }
}

impl From<maxhq_client::TransferState> for TransferState {
    fn from(state: maxhq_client::TransferState) -> Self {
        match state {
            maxhq_client::TransferState::Uploading => Self::Uploading,
            maxhq_client::TransferState::Waiting => Self::Waiting,
            maxhq_client::TransferState::Paused => Self::Paused,
            maxhq_client::TransferState::Done => Self::Done,
            maxhq_client::TransferState::Failed => Self::Failed,
        }
    }
}

/// Where a download landed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct Downloaded {
    /// The file or folder on this machine, under `~/Downloads/MaxHQ`.
    pub path: String,
    /// The links in it this machine couldn't make, by their paths inside
    /// it. Only Windows leaves any out, where neither Developer Mode nor
    /// an administrator lets it make links.
    pub skipped_links: Vec<String>,
}

/// A picture or video for the timeline, as the media cache has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MediaFile {
    /// It's in the cache under `name`, which the webview loads through the
    /// media scheme.
    Cached {
        /// The file's name in the cache.
        name: String,
        /// Its size in bytes.
        #[specta(type = specta_typescript::Number)]
        size: u64,
    },
    /// It's larger than the limit asked, so it stayed on the device.
    TooLarge {
        /// Its size in bytes.
        #[specta(type = specta_typescript::Number)]
        size: u64,
    },
    /// The device refused its path.
    Refused {
        /// Whether it's outside the agent's folders, which Show anyway can
        /// still fetch. A `maxd` from before Show anyway never says so.
        outside: bool,
        /// Why, for a person to read.
        message: String,
    },
}

impl From<Media> for MediaFile {
    fn from(media: Media) -> Self {
        match media {
            Media::Cached { name, size } => Self::Cached { name, size },
            Media::TooLarge { size } => Self::TooLarge { size },
            Media::Refused { outside, message } => Self::Refused { outside, message },
        }
    }
}

/// Where a copy of a project goes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CopyTarget {
    /// A new container workspace on a device, which starts when the
    /// device does, as a new workspace does by default.
    Container {
        /// The device whose Incus runs it.
        device: DeviceId,
        /// The container's name after `mhq-`. The project's name when unset.
        name: Option<String>,
    },
    /// A clone into a new folder on a device's own disk.
    Folder {
        /// The device.
        device: DeviceId,
        /// The folder the clone goes in, which has to be there already.
        parent: String,
        /// The new folder's name.
        name: String,
    },
}

/// What a copy of a project made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct CopiedProject {
    /// The new workspace.
    pub workspace: WorkspaceId,
    /// How many of the chosen files went in.
    pub written: u32,
    /// The chosen files that didn't, and why.
    pub failed: Vec<PlaceFailure>,
}

/// Hears each step of one transfer. A window that went away misses the
/// rest, and the transfer goes on.
pub trait Report: Fn(TransferProgress) + Send + Sync + 'static {}

impl<F: Fn(TransferProgress) + Send + Sync + 'static> Report for F {}

impl<C: Connector> App<C> {
    /// Uploads a file or folder you handed over to `target` on a device,
    /// and gives what the device made of it. `report` hears how far it has
    /// got.
    pub async fn upload(
        &self,
        device: DeviceId,
        target: UploadTarget,
        path: String,
        report: impl Report,
    ) -> Result<ActionOutcome, CommandError> {
        let path = self
            .offered
            .check(&path)
            .map_err(|error| CommandError::Failed {
                message: error.to_string(),
            })?;
        self.devices
            .upload(device, target, path, reporter(report))
            .await
    }

    /// Uploads pasted bytes to `target` on a device as one file called
    /// `name`. `data` is the bytes in base64.
    pub async fn upload_pasted(
        &self,
        device: DeviceId,
        target: UploadTarget,
        name: String,
        mime: String,
        data: String,
        report: impl Report,
    ) -> Result<ActionOutcome, CommandError> {
        let bytes = STANDARD
            .decode(data.as_bytes())
            .map_err(|_| CommandError::Failed {
                message: "The pasted data isn't base64".to_owned(),
            })?;
        self.devices
            .upload_bytes(device, target, name, mime, bytes, reporter(report))
            .await
    }

    /// Fetches a file or folder an agent can see into `~/Downloads/MaxHQ`,
    /// under a name nothing there has yet. `report` hears how far it has
    /// got.
    pub async fn download(
        &self,
        agent: AgentId,
        path: String,
        report: impl Report,
    ) -> Result<Downloaded, CommandError> {
        let landed = self.devices.download(agent, path, reporter(report)).await?;
        Ok(Downloaded {
            path: landed.path.to_string_lossy().into_owned(),
            skipped_links: landed.skipped_links,
        })
    }

    /// Fetches a picture or video an agent can see into the media cache,
    /// unless it's there already or is larger than `limit` bytes. A limit
    /// of 0 only asks how large it is. `anyway` asks for one file outside
    /// the agent's folders, and only your click on Show anyway sets it.
    /// `report` hears how far it has got.
    pub async fn media(
        &self,
        agent: AgentId,
        path: String,
        limit: u32,
        anyway: bool,
        report: impl Report,
    ) -> Result<MediaFile, CommandError> {
        let fetched = self
            .devices
            .media(agent, path, u64::from(limit), anyway, reporter(report))
            .await?;
        Ok(fetched.into())
    }

    /// Readies a file or folder an agent can see, such as a picture in its
    /// timeline, for an agent on `device`, as [`attach_from_agent`] tells.
    /// `report` hears both legs, each under its own transfer.
    pub async fn attach_from_agent(
        &self,
        device: DeviceId,
        target: Option<UploadTarget>,
        agent: AgentId,
        path: String,
        report: impl Report,
    ) -> Result<ActionOutcome, CommandError> {
        attach_from_agent(&self.devices, device, target, agent, path, reporter(report)).await
    }

    /// Copies the project of workspace `from` to `target`, as
    /// [`copy_project`] tells. `report` hears the files' transfers.
    pub async fn copy_project(
        &self,
        from: WorkspaceId,
        target: CopyTarget,
        files: Vec<String>,
        report: impl Report,
    ) -> Result<CopiedProject, CommandError> {
        copy_project(
            &self.devices,
            &self.view,
            from,
            target,
            files,
            reporter(report),
        )
        .await
    }
}

/// Readies a file or folder you handed over on a device, for an agent
/// there to read. A remote device that takes files gets a copy in its
/// staging area. This machine reads it where it is, and so does a device
/// that doesn't take files, as before there were transfers.
pub async fn stage_path<C: Connector>(
    devices: &Devices<C>,
    device: DeviceId,
    path: String,
) -> Result<ActionOutcome, CommandError> {
    let remote = !devices.is_local(device);
    if remote
        && devices
            .capabilities(device)
            .is_some_and(|does| does.transfers)
    {
        let quiet: OnProgress = Arc::new(|_| {});
        devices
            .upload(device, UploadTarget::Device, PathBuf::from(path), quiet)
            .await
    } else {
        devices.stage(device, AttachmentSource::Path { path }).await
    }
}

/// Readies a file or folder agent `from` can see for an agent on `device`.
/// It comes through `from`'s device into a copy on this machine, so that
/// device decides what `from` can see, as it does for a download. The copy
/// then uploads to `target` when one is given, and otherwise stages as
/// [`stage_path`] does. It goes once `device` has it.
pub async fn attach_from_agent<C: Connector>(
    devices: &Devices<C>,
    device: DeviceId,
    target: Option<UploadTarget>,
    from: AgentId,
    path: String,
    on_progress: OnProgress,
) -> Result<ActionOutcome, CommandError> {
    let copied = devices.copy(from, path, Arc::clone(&on_progress)).await?;
    let path = copied.path().to_owned();
    match target {
        Some(target) => devices.upload(device, target, path, on_progress).await,
        None => stage_path(devices, device, path.to_string_lossy().into_owned()).await,
    }
}

/// Copies the project of workspace `from` to `target`. The target clones
/// it from the project's remote, and once the new workspace is ready the
/// `files`, paths in `from`'s checkout, come into a copy here, go to the
/// target's device, and move into the new checkout. Files there already
/// stay as they are. The copy goes once the device has the files.
pub async fn copy_project<C: Connector>(
    devices: &Devices<C>,
    view: &View,
    from: WorkspaceId,
    target: CopyTarget,
    files: Vec<String>,
    on_progress: OnProgress,
) -> Result<CopiedProject, CommandError> {
    let (source, project) =
        view.read(
            |merge| match merge.store().get(&EntityKey::Workspace(from)) {
                Some(Entity::Workspace(found)) => match found.project {
                    Some(project) => Ok((found.device, project)),
                    None => Err(CommandError::Failed {
                        message: "This workspace has no project to copy".to_owned(),
                    }),
                },
                _ => Err(CommandError::NotFound {
                    what: "This workspace".to_owned(),
                }),
            },
        )?;
    devices.check_copies(source)?;
    let key = IdempotencyKey::generate();
    let (device, start) = match target {
        CopyTarget::Container { device, name } => {
            let spec = WorkspaceSpec {
                project,
                device,
                name,
                start_at_boot: true,
            };
            (device, ClientMessage::CreateWorkspace { key, spec })
        }
        CopyTarget::Folder {
            device,
            parent,
            name,
        } => {
            let clone = ClientMessage::CloneFolder {
                key,
                project,
                device,
                parent,
                name,
            };
            (device, clone)
        }
    };
    devices.check_copies(device)?;
    let workspace = match devices.act(start).await? {
        ActionOutcome::Created { workspace } => workspace,
        other => return Err(lookup_failed(other)),
    };
    let mut seen = false;
    view.wait_for(|merge| ready(merge, workspace, &mut seen))
        .await?;
    if files.is_empty() {
        return Ok(CopiedProject {
            workspace,
            written: 0,
            failed: Vec::new(),
        });
    }

    let copied = devices
        .copy_files(from, files, Arc::clone(&on_progress))
        .await?;
    // Links this machine couldn't make don't go on, so they show with the
    // files that failed.
    let unmade: Vec<PlaceFailure> = copied
        .skipped_links()
        .iter()
        .map(|path| PlaceFailure {
            path: path.clone(),
            reason: "this computer couldn't make the link".to_owned(),
        })
        .collect();
    let into = UploadTarget::Workspace { workspace };
    let uploaded = devices
        .upload(device, into, copied.path().to_owned(), on_progress)
        .await;
    drop(copied);
    let attachment = match uploaded? {
        ActionOutcome::Staged { attachment } => attachment.id,
        other => return Err(lookup_failed(other)),
    };
    let place = ClientMessage::PlaceStaged {
        key: IdempotencyKey::generate(),
        workspace,
        attachment,
        overwrite: false,
    };
    match devices.act(place).await? {
        ActionOutcome::Placed {
            written,
            mut failed,
        } => {
            failed.extend(unmade);
            Ok(CopiedProject {
                workspace,
                written,
                failed,
            })
        }
        other => Err(lookup_failed(other)),
    }
}

/// Whether a new workspace is ready for files: a container running with
/// its guest up, or a folder whose clone is done. None while it gets
/// there, which includes before its device first reports it. `seen`
/// remembers that it did, so a workspace that goes away fails.
fn ready(
    merge: &Merge,
    workspace: WorkspaceId,
    seen: &mut bool,
) -> Option<Result<(), CommandError>> {
    let fail = |message: &str| {
        Some(Err(CommandError::Failed {
            message: message.to_owned(),
        }))
    };
    let Some(Entity::Workspace(found)) = merge.store().get(&EntityKey::Workspace(workspace)) else {
        return if *seen {
            fail("The new workspace went away before it was ready")
        } else {
            None
        };
    };
    *seen = true;
    if let Some(clone) = &found.clone {
        return match &clone.state {
            CloneState::Cloning { .. } => None,
            CloneState::Failed { reason } => Some(Err(CommandError::Failed {
                message: reason.clone(),
            })),
            CloneState::Cancelled => fail("The clone was cancelled"),
        };
    }
    let Some(managed) = &found.managed else {
        return Some(Ok(()));
    };
    match &managed.state {
        WorkspaceState::Creating { .. } | WorkspaceState::Starting => None,
        WorkspaceState::Running => {
            matches!(managed.guest, GuestState::Connected { .. }).then_some(Ok(()))
        }
        WorkspaceState::Failed { reason } => Some(Err(CommandError::Failed {
            message: reason.clone(),
        })),
        _ => fail("The new workspace stopped before it was ready"),
    }
}

/// Stages each path you handed over with `stage`, in order, and fails the
/// others.
pub(crate) async fn stage_each<F, Staged>(
    offered: &Offered,
    paths: Vec<String>,
    stage: F,
) -> Vec<ActionReply>
where
    F: Fn(String) -> Staged,
    Staged: Future<Output = Result<ActionOutcome, CommandError>>,
{
    let mut replies = Vec::with_capacity(paths.len());
    for path in paths {
        let staged = match offered.check(&path) {
            Ok(_) => stage(path).await,
            Err(error) => Err(CommandError::Failed {
                message: error.to_string(),
            }),
        };
        replies.push(staged.into());
    }
    replies
}

/// Hands each step of a transfer to `report` in the shape a window reads.
fn reporter(report: impl Report) -> OnProgress {
    Arc::new(move |progress| report(TransferProgress::from(progress)))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use maxhq_client::LinkEvent;
    use maxhq_protocol::ids::{AttachmentId, LinkId, ProjectId};
    use maxhq_protocol::message::Capabilities;
    use maxhq_protocol::model::{
        CloneProgress, CloneStage, EgressCheck, ManagedWorkspace, Workspace, WorkspaceLocation,
    };
    use maxhq_protocol::state::Change;
    use maxhq_protocol::time::Timestamp;
    use maxhq_protocol::timeline::{AttachmentKind, AttachmentRef};
    use maxhq_protocol::ClientMessage;

    use super::*;
    use crate::devices::tests::{connect_doing, two_devices, wait_until, Call, Harness};
    use crate::tests::block_on;

    /// A report that keeps the JSON a window would hear.
    fn recording() -> (impl Report, Arc<Mutex<Vec<String>>>) {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let keep = heard.clone();
        let report = move |progress: TransferProgress| {
            let json = serde_json::to_string(&progress).unwrap();
            keep.lock().unwrap().push(json);
        };
        (report, heard)
    }

    /// The states a report heard, in order.
    fn states(heard: &Mutex<Vec<String>>) -> Vec<String> {
        heard
            .lock()
            .unwrap()
            .iter()
            .map(|json| {
                let (_, state) = json.split_once(r#""state":""#).unwrap();
                state.split('"').next().unwrap().to_owned()
            })
            .collect()
    }

    fn does(transfers: bool) -> Capabilities {
        Capabilities {
            transfers,
            tunnels: true,
            workspaces: true,
            github_checks: true,
            inbox: false,
            ..Capabilities::default()
        }
    }

    fn uploads(calls: &[(LinkId, Call)]) -> Vec<(LinkId, UploadTarget, PathBuf)> {
        calls
            .iter()
            .filter_map(|(link, call)| match call {
                Call::Upload(target, path) => Some((*link, target.clone(), path.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_remote_device_that_takes_files_gets_a_copy_and_this_machine_reads_in_place() {
        let (h, local, nl) = two_devices();
        let (mac, nl_device) = (DeviceId::derive("mac"), DeviceId::derive("nl"));
        connect_doing(&h, &local, "mac", does(true));
        connect_doing(&h, &nl, "nl", does(true));
        h.fake.calls();

        block_on(stage_path(
            &h.devices,
            nl_device,
            "/Users/max/notes.txt".into(),
        ))
        .unwrap();
        assert_eq!(
            uploads(&h.fake.calls()),
            [(
                nl.id,
                UploadTarget::Device,
                PathBuf::from("/Users/max/notes.txt")
            )]
        );

        // This machine takes files too, and still reads the path itself.
        block_on(stage_path(&h.devices, mac, "/Users/max/notes.txt".into())).unwrap();
        let calls = h.fake.calls();
        assert!(uploads(&calls).is_empty());
        assert!(matches!(
            &calls[..],
            [(id, Call::Send(ClientMessage::StageAttachment { .. }))] if *id == local.id
        ));

        // A device that doesn't take files gets the path, as before.
        connect_doing(&h, &nl, "nl", does(false));
        h.fake.calls();
        block_on(stage_path(
            &h.devices,
            nl_device,
            "/Users/max/notes.txt".into(),
        ))
        .unwrap();
        let calls = h.fake.calls();
        assert!(uploads(&calls).is_empty());
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn a_transfers_report_hears_its_progress_then_done() {
        let (h, _, nl) = two_devices();
        let (report, heard) = recording();
        let outcome = block_on(h.devices.upload(
            DeviceId::derive("nl"),
            UploadTarget::Agent {
                agent: AgentId::derive("b"),
            },
            PathBuf::from("/Users/max/site"),
            reporter(report),
        ));
        assert_eq!(outcome, Ok(ActionOutcome::Done));
        assert_eq!(states(&heard), ["uploading", "done"]);
        let last = heard.lock().unwrap().last().cloned().unwrap();
        assert!(
            last.contains(r#""sent":4,"total":4,"file":"site""#),
            "{last}"
        );
        assert_eq!(uploads(&h.fake.calls())[0].0, nl.id);

        let (report, heard) = recording();
        let landed = block_on(h.devices.download(
            AgentId::derive("b"),
            "dist/app.js".into(),
            reporter(report),
        ));
        let landed = landed.map(|saved| (saved.path, saved.skipped_links));
        assert_eq!(
            landed,
            Ok((PathBuf::from("/downloads/dist/app.js"), Vec::new()))
        );
        assert_eq!(states(&heard), ["uploading", "done"]);
    }

    #[test]
    fn a_picture_comes_through_the_agents_device_and_one_past_the_limit_stays() {
        let (h, _, nl) = two_devices();
        let agent = AgentId::derive("b");
        let fetched = block_on(h.devices.media(
            agent,
            "shots/home.png".into(),
            1024,
            false,
            Arc::new(|_| {}),
        ))
        .map(MediaFile::from);
        assert_eq!(
            fetched,
            Ok(MediaFile::Cached {
                name: "home.png".into(),
                size: 4
            })
        );
        let json = serde_json::to_string(&fetched.unwrap()).unwrap();
        assert_eq!(json, r#"{"kind":"cached","name":"home.png","size":4}"#);

        let probed = block_on(
            h.devices
                .media(agent, "clip.mp4".into(), 0, true, Arc::new(|_| {})),
        )
        .map(MediaFile::from);
        assert_eq!(probed, Ok(MediaFile::TooLarge { size: 4 }));
        let calls: Vec<_> = h
            .fake
            .calls()
            .into_iter()
            .filter(|(_, call)| matches!(call, Call::Media(..)))
            .collect();
        assert_eq!(
            calls,
            [
                (
                    nl.id,
                    Call::Media(agent, "shots/home.png".into(), 1024, false)
                ),
                (nl.id, Call::Media(agent, "clip.mp4".into(), 0, true)),
            ]
        );
    }

    #[test]
    fn a_cancelled_transfer_stops_and_its_report_hears_it_failed() {
        let (h, _, _) = two_devices();
        h.fake.record().hold = true;
        let (report, heard) = recording();
        let outcome = std::thread::scope(|scope| {
            let upload = scope.spawn(|| {
                block_on(h.devices.upload(
                    DeviceId::derive("nl"),
                    UploadTarget::Device,
                    PathBuf::from("/Users/max/big.iso"),
                    reporter(report),
                ))
            });
            wait_until("the upload starts", || !h.fake.record().held.is_empty());
            let transfer = *h.fake.record().held.keys().next().unwrap();
            h.devices.cancel_transfer(transfer);
            upload.join().unwrap()
        });
        assert_eq!(
            outcome,
            Err(CommandError::Failed {
                message: "Cancelled".into()
            })
        );
        assert_eq!(states(&heard), ["uploading", "failed"]);
        // Every link heard the cancel, and only the one carrying it acted.
        let cancels = h
            .fake
            .calls()
            .into_iter()
            .filter(|(_, call)| matches!(call, Call::Cancel(_)))
            .count();
        assert_eq!(cancels, 2);
    }

    #[test]
    fn only_offered_paths_reach_the_device() {
        let offered = Offered::default();
        offered.offer([PathBuf::from("/tmp/a.png"), PathBuf::from("/tmp/c")]);
        let staged = Mutex::new(Vec::new());
        let paths = ["/tmp/a.png", "/tmp/b.png", "/tmp/c"].map(String::from);
        let replies = block_on(stage_each(&offered, paths.into(), |path| {
            staged.lock().unwrap().push(path);
            async { Ok(ActionOutcome::Done) }
        }));
        let done = ActionReply::Ok {
            data: ActionOutcome::Done,
        };
        let refused = ActionReply::Error {
            error: CommandError::Failed {
                message: "/tmp/b.png wasn't dropped, pasted or picked".to_owned(),
            },
        };
        assert_eq!(replies, [done.clone(), refused, done]);
        assert_eq!(*staged.lock().unwrap(), ["/tmp/a.png", "/tmp/c"]);
    }

    #[test]
    fn a_file_another_agent_can_see_comes_through_its_device_then_goes_on() {
        let (h, local, nl) = two_devices();
        let (mac, nl_device) = (DeviceId::derive("mac"), DeviceId::derive("nl"));
        let (a, b) = (AgentId::derive("a"), AgentId::derive("b"));
        connect_doing(&h, &local, "mac", does(true));
        connect_doing(&h, &nl, "nl", does(true));
        h.fake.calls();

        // From the remote agent to one on this machine, which stages the
        // copy where it is.
        let (report, heard) = recording();
        let outcome = block_on(attach_from_agent(
            &h.devices,
            mac,
            None,
            b,
            "shots/home.png".into(),
            reporter(report),
        ));
        assert!(outcome.is_ok(), "{outcome:?}");
        let calls = h.fake.calls();
        let [(fetched, Call::Copy(agent, path)), (staged, Call::Send(stage))] = &calls[..] else {
            panic!("expected a copy then a stage, got {calls:?}");
        };
        assert_eq!(
            (*fetched, *agent, path.as_str()),
            (nl.id, b, "shots/home.png")
        );
        let ClientMessage::StageAttachment {
            source: AttachmentSource::Path { path },
            ..
        } = stage
        else {
            panic!("expected a stage by path, got {stage:?}");
        };
        assert_eq!(*staged, local.id);
        assert!(
            path.starts_with("/copies/") && path.ends_with("/home.png"),
            "{path}"
        );
        assert_eq!(states(&heard), ["uploading", "done"]);

        // From this machine's agent to the remote one's folder, which
        // uploads the copy.
        let (report, heard) = recording();
        let target = UploadTarget::Agent { agent: b };
        let outcome = block_on(attach_from_agent(
            &h.devices,
            nl_device,
            Some(target.clone()),
            a,
            "/Users/max/site/plan.pdf".into(),
            reporter(report),
        ));
        assert_eq!(outcome, Ok(ActionOutcome::Done));
        let calls = h.fake.calls();
        assert_eq!(
            calls[0],
            (local.id, Call::Copy(a, "/Users/max/site/plan.pdf".into()))
        );
        let [(through, to, path)] = &uploads(&calls)[..] else {
            panic!("expected one upload, got {calls:?}");
        };
        assert_eq!((*through, to), (nl.id, &target));
        assert!(path.starts_with("/copies") && path.ends_with("plan.pdf"));
        assert_eq!(states(&heard), ["uploading", "done", "uploading", "done"]);
    }

    #[test]
    fn a_file_the_agents_device_refuses_goes_nowhere() {
        let (h, _, nl) = two_devices();
        h.fake.record().unsupported.insert(nl.id);
        let outcome = block_on(attach_from_agent(
            &h.devices,
            DeviceId::derive("mac"),
            None,
            AgentId::derive("b"),
            "shots/home.png".into(),
            Arc::new(|_| {}),
        ));
        assert!(outcome.is_err(), "{outcome:?}");
        let calls = h.fake.calls();
        assert!(
            calls.iter().all(|(_, call)| matches!(call, Call::Copy(..))),
            "{calls:?}"
        );
    }

    #[test]
    fn a_device_that_doesnt_take_files_refuses_an_upload() {
        let (h, _, nl) = two_devices();
        h.fake.record().unsupported.insert(nl.id);
        let outcome = block_on(h.devices.upload_bytes(
            DeviceId::derive("nl"),
            UploadTarget::Device,
            "paste.png".into(),
            "image/png".into(),
            vec![1, 2, 3],
            Arc::new(|_| {}),
        ));
        assert_eq!(
            outcome,
            Ok(ActionOutcome::Refused {
                reason: "The device doesn't do file transfers".into()
            })
        );
    }

    /// The workspace a copy makes, on `device`.
    fn made(device: &str, location: WorkspaceLocation) -> Workspace {
        Workspace {
            id: WorkspaceId::derive("made"),
            project: Some(ProjectId::derive("p")),
            device: DeviceId::derive(device),
            location,
            managed: None,
            clone: None,
        }
    }

    /// A folder on this machine a copy clones into, as far as `clone` says.
    fn folder(clone: Option<CloneState>) -> Workspace {
        let location = WorkspaceLocation::Folder {
            path: "/Users/dev/src/site".into(),
        };
        Workspace {
            clone: clone.map(|state| CloneProgress { state }),
            ..made("mac", location)
        }
    }

    /// A container on devbox a copy makes, in `state` with its guest `guest`.
    fn container(state: WorkspaceState, guest: GuestState) -> Workspace {
        let location = WorkspaceLocation::Container {
            instance: "mhq-site".into(),
            path: "/home/agent/site".into(),
        };
        Workspace {
            managed: Some(ManagedWorkspace {
                state,
                start_at_boot: true,
                guest,
                egress: EgressCheck::NotRun,
                recipe: 1,
                created_at: Timestamp(1),
                ssh_host_key: None,
                unsaved: None,
                memory_soft: None,
            }),
            ..made("nl", location)
        }
    }

    /// Has a link report that a workspace is now as given.
    fn report(h: &Harness, link: LinkId, workspace: Workspace) {
        let value = Entity::Workspace(workspace);
        h.report(link, LinkEvent::Changed(vec![Change::Upsert { value }]));
    }

    fn staged() -> AttachmentRef {
        AttachmentRef {
            id: AttachmentId::derive("files"),
            name: "files".into(),
            kind: AttachmentKind::Folder,
            bytes: 7,
            mime: None,
            path: "/var/lib/maxhq/staging/files".into(),
            thumbnail: None,
        }
    }

    /// Waits until the copy behind `path` has gone.
    fn wait_gone(path: &std::path::Path) {
        let folder = path.parent().unwrap().to_owned();
        wait_until("the copy here never went", || !folder.exists());
    }

    #[test]
    fn a_copy_to_a_folder_waits_for_the_clone_then_places_the_files_and_drops_its_copy() {
        let (h, local, nl) = two_devices();
        let made = WorkspaceId::derive("made");
        let refused = PlaceFailure {
            path: "app/.env.local".into(),
            reason: "It's there already".into(),
        };
        {
            let mut record = h.fake.record();
            record.acts.extend([
                ActionOutcome::Created { workspace: made },
                ActionOutcome::Placed {
                    written: 1,
                    failed: vec![refused.clone()],
                },
            ]);
            record.uploaded = Some(ActionOutcome::Staged {
                attachment: staged(),
            });
        }
        let from = WorkspaceId::derive("w-nl");
        let files = vec![".env".to_owned(), "app/.env.local".to_owned()];
        let target = CopyTarget::Folder {
            device: DeviceId::derive("mac"),
            parent: "/Users/dev/src".into(),
            name: "site".into(),
        };

        let copied = std::thread::scope(|scope| {
            let copy = scope.spawn(|| {
                let quiet: OnProgress = Arc::new(|_| {});
                block_on(copy_project(
                    &h.devices,
                    &h.view,
                    from,
                    target,
                    files.clone(),
                    quiet,
                ))
            });
            wait_until("the clone never started", || {
                h.fake.record().acts.len() == 1
            });
            let cloning = CloneState::Cloning {
                stage: CloneStage::Receiving,
                percent: Some(42),
                stalled: false,
            };
            report(&h, local.id, folder(Some(cloning)));
            let calls = h.fake.calls();
            let [(
                link,
                Call::Send(ClientMessage::CloneFolder {
                    project,
                    parent,
                    name,
                    ..
                }),
            )] = &calls[..]
            else {
                panic!("only the clone goes out while it runs: {calls:?}");
            };
            assert_eq!(*link, local.id);
            assert_eq!(*project, ProjectId::derive("p"));
            assert_eq!((parent.as_str(), name.as_str()), ("/Users/dev/src", "site"));

            report(&h, local.id, folder(None));
            copy.join().unwrap()
        });

        assert_eq!(
            copied,
            Ok(CopiedProject {
                workspace: made,
                written: 1,
                failed: vec![refused],
            })
        );
        let calls = h.fake.calls();
        let [(fetched, Call::CopyFiles(of, paths)), (uploaded, Call::Upload(into, path)), (placed, Call::Send(place))] =
            &calls[..]
        else {
            panic!("the files come here, go to the folder's device, then move in: {calls:?}");
        };
        assert_eq!((*fetched, *of, paths), (nl.id, from, &files));
        assert_eq!(*uploaded, local.id);
        assert_eq!(*into, UploadTarget::Workspace { workspace: made });
        assert!(matches!(
            place,
            ClientMessage::PlaceStaged { workspace, attachment, overwrite: false, .. }
                if *workspace == made && *attachment == staged().id
        ));
        assert_eq!(*placed, local.id);
        wait_gone(path);
    }

    #[test]
    fn a_copy_to_a_container_waits_for_its_guest_and_a_failed_upload_still_drops_its_copy() {
        let (h, local, nl) = two_devices();
        let made = WorkspaceId::derive("made");
        {
            let mut record = h.fake.record();
            record
                .acts
                .push_back(ActionOutcome::Created { workspace: made });
            record.unsupported.insert(nl.id);
        }
        let from = WorkspaceId::derive("w");
        let files = vec![".env".to_owned()];
        let target = CopyTarget::Container {
            device: DeviceId::derive("nl"),
            name: Some("site".into()),
        };

        let copied = std::thread::scope(|scope| {
            let copy = scope.spawn(|| {
                let quiet: OnProgress = Arc::new(|_| {});
                block_on(copy_project(
                    &h.devices,
                    &h.view,
                    from,
                    target,
                    files.clone(),
                    quiet,
                ))
            });
            wait_until("the workspace was never made", || {
                h.fake.record().acts.is_empty()
            });
            let creating = WorkspaceState::Creating {
                step: "Cloning owner/name, 42%".into(),
                done: 2,
                of: 5,
            };
            report(&h, nl.id, container(creating, GuestState::Installing));
            report(
                &h,
                nl.id,
                container(WorkspaceState::Running, GuestState::Installing),
            );
            let calls = h.fake.calls();
            let [(link, Call::Send(ClientMessage::CreateWorkspace { spec, .. }))] = &calls[..]
            else {
                panic!("nothing moves before the guest is up: {calls:?}");
            };
            assert_eq!(*link, nl.id);
            assert_eq!(
                *spec,
                WorkspaceSpec {
                    project: ProjectId::derive("p"),
                    device: DeviceId::derive("nl"),
                    name: Some("site".into()),
                    start_at_boot: true,
                }
            );

            let up = GuestState::Connected {
                version: "0.1.0".into(),
            };
            report(&h, nl.id, container(WorkspaceState::Running, up));
            copy.join().unwrap()
        });

        assert_eq!(
            copied,
            Err(CommandError::Failed {
                message: "The device doesn't do file transfers".into()
            })
        );
        let calls = h.fake.calls();
        let [(fetched, Call::CopyFiles(of, _)), (uploaded, Call::Upload(_, path))] = &calls[..]
        else {
            panic!("the files come here and the upload is refused: {calls:?}");
        };
        assert_eq!((*fetched, *of, *uploaded), (local.id, from, nl.id));
        wait_gone(path);
    }

    #[test]
    fn a_refused_start_or_a_failed_clone_ends_a_copy_before_any_files_move() {
        let (h, local, nl) = two_devices();
        let made = WorkspaceId::derive("made");
        let from = WorkspaceId::derive("w-nl");
        let quiet: OnProgress = Arc::new(|_| {});
        let target = CopyTarget::Folder {
            device: DeviceId::derive("mac"),
            parent: "/Users/dev/src".into(),
            name: "site".into(),
        };
        let copy = || {
            copy_project(
                &h.devices,
                &h.view,
                from,
                target.clone(),
                vec![".env".to_owned()],
                Arc::clone(&quiet),
            )
        };

        connect_doing(&h, &nl, "nl", does(true));
        assert_eq!(
            block_on(copy()),
            Err(CommandError::Failed {
                message: "devbox's maxd is too old to copy projects. Update it first".into()
            })
        );
        connect_doing(
            &h,
            &nl,
            "nl",
            Capabilities {
                copy: true,
                ..does(true)
            },
        );

        let there = ActionOutcome::Refused {
            reason: "/Users/dev/src/site is there already".into(),
        };
        h.fake.record().acts.push_back(there);
        assert_eq!(
            block_on(copy()),
            Err(CommandError::Failed {
                message: "/Users/dev/src/site is there already".into()
            })
        );

        let reason = "git clone failed: Repository not found".to_owned();
        let failed = CloneState::Failed {
            reason: reason.clone(),
        };
        report(&h, local.id, folder(Some(failed)));
        h.fake
            .record()
            .acts
            .push_back(ActionOutcome::Created { workspace: made });
        assert_eq!(
            block_on(copy()),
            Err(CommandError::Failed { message: reason })
        );

        report(&h, local.id, folder(Some(CloneState::Cancelled)));
        h.fake
            .record()
            .acts
            .push_back(ActionOutcome::Created { workspace: made });
        assert_eq!(
            block_on(copy()),
            Err(CommandError::Failed {
                message: "The clone was cancelled".into()
            })
        );
        let calls = h.fake.calls();
        assert!(
            calls
                .iter()
                .all(|(_, call)| matches!(call, Call::Send(ClientMessage::CloneFolder { .. }))),
            "{calls:?}"
        );
        assert_eq!(calls.len(), 3);
    }
}
