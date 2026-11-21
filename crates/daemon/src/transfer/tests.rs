// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Transfers end to end: a client behind a wire writer, this device's
//! disk, and a fake container whose guest runs on a folder of its own.
//! Paths cross the fake guest link as the container would see them.

use std::fs::{self, Permissions};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use maxhq_core::skills::disk::DiskFs;
use maxhq_core::skills::store::FolderStore;
use maxhq_protocol::frame::{Frame, FrameDecoder, MAX_TRANSFER_CHUNK};
use maxhq_protocol::ids::DeviceId;
use maxhq_protocol::message::{EntryKind, IgnoredFile, PlaceFailure, SHOW_ANYWAY_SINCE};
use maxhq_protocol::PROTOCOL_VERSION;
use maxhq_transfer::manifest::Layout;
use maxhq_transfer::{Outgoing, Source, Walk};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio::time::timeout;

use super::*;
use crate::doctext::Extractor;
use crate::guest::files::{self, Files, Out, WORK_DIR};
use crate::session::holders::{AgentFiles, DeliverError};
use crate::testing::TempDir;

mod pull;

/// How long a test waits for anything before failing.
const WAIT: Duration = Duration::from_secs(10);

/// Room checks that never refuse.
const ANY_ROOM: Room = Room {
    keep_bytes: 0,
    keep_percent: 0,
    keep_at_most: 0,
};

/// Bytes that differ from one offset to the next.
fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|at| (at * 31 % 251) as u8).collect()
}

/// What a client got.
#[derive(Debug)]
enum Got {
    Message(ServerMessage),
    Chunk {
        transfer: TransferId,
        offset: u64,
        data: Vec<u8>,
    },
}

/// One client connection, as transfers see it.
struct Client {
    route: Route,
    got: mpsc::UnboundedReceiver<Got>,
}

impl Client {
    fn new() -> Self {
        let (ours, mut theirs) = tokio::io::duplex(1024 * 1024);
        let (wire, _writer) = maxhq_wire::spawn(ours);
        let (tx, got) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut decoder = FrameDecoder::new();
            let mut buf = vec![0; 64 * 1024];
            loop {
                let n = match theirs.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                decoder.push(&buf[..n]);
                while let Some(frame) = decoder.next_frame().unwrap() {
                    let got = match frame {
                        Frame::Message(json) => Got::Message(serde_json::from_slice(json).unwrap()),
                        Frame::TransferChunk {
                            transfer,
                            offset,
                            data,
                        } => Got::Chunk {
                            transfer,
                            offset,
                            data: data.to_vec(),
                        },
                        other => panic!("a transfer sent {other:?}"),
                    };
                    if tx.send(got).is_err() {
                        return;
                    }
                }
            }
        });
        Self {
            route: Route::new(wire),
            got,
        }
    }

    async fn next(&mut self) -> Got {
        timeout(WAIT, self.got.recv())
            .await
            .expect("something in time")
            .expect("the wire stays up")
    }

    /// The next message, past any chunks.
    async fn message(&mut self) -> ServerMessage {
        loop {
            if let Got::Message(message) = self.next().await {
                return message;
            }
        }
    }
}

/// A client's side of one upload.
struct Upload {
    key: IdempotencyKey,
    transfer: TransferId,
    target: UploadTarget,
    walk: Walk,
    source: Source,
    outgoing: Outgoing,
    /// Flips a byte of the first chunk at offset 0, once.
    corrupt: bool,
}

impl Upload {
    fn new(target: UploadTarget, path: &Path) -> Self {
        let walk = maxhq_transfer::walk(path).unwrap();
        Self {
            key: IdempotencyKey::generate(),
            transfer: TransferId::generate(),
            target,
            source: Source::new(&walk),
            outgoing: Outgoing::new(walk.total()),
            walk,
            corrupt: false,
        }
    }

    fn begin(&self, transfers: &Transfers, client: &Client) {
        transfers.upload_begin(
            &client.route,
            self.key,
            self.transfer,
            self.target.clone(),
            UploadSource::Stream {
                name: self.walk.name.clone(),
                entries: self.walk.entries.clone(),
            },
        );
    }

    /// Sends what the credit allows, up to `stop`.
    fn send(&mut self, transfers: &Transfers, stop: u64) {
        while let Some((offset, len)) = self.outgoing.next(MAX_TRANSFER_CHUNK) {
            if offset >= stop {
                return;
            }
            let len = len.min(usize::try_from(stop - offset).unwrap_or(usize::MAX));
            let mut data = self.source.read(offset, len).unwrap();
            if offset == 0 && std::mem::take(&mut self.corrupt) {
                data[0] ^= 0xff;
            }
            transfers.chunk(self.transfer, offset, data);
            self.outgoing.sent_chunk(len);
        }
    }

    /// Takes a credit, the first after a begin as where to pick up.
    fn credit(&mut self, message: &ServerMessage, fresh: &mut bool) -> Option<u64> {
        let ServerMessage::TransferCredit {
            transfer,
            committed,
            window_end,
        } = *message
        else {
            return None;
        };
        assert_eq!(transfer, self.transfer);
        if std::mem::take(fresh) {
            self.outgoing.resume(committed, window_end).unwrap();
        } else {
            self.outgoing.credit(committed, window_end).unwrap();
        }
        Some(committed)
    }

    /// Sends as credit comes until the action's result, sending the begin
    /// again after a hash mismatch as a client does. Gives everything the
    /// device said.
    async fn finish(
        &mut self,
        transfers: &Transfers,
        client: &mut Client,
        mut fresh: bool,
    ) -> Vec<ServerMessage> {
        let mut said = Vec::new();
        loop {
            let message = client.message().await;
            if self.credit(&message, &mut fresh).is_some() {
                self.send(transfers, u64::MAX);
            }
            if let ServerMessage::TransferFailed {
                code: ErrorCode::HashMismatch,
                ..
            } = message
            {
                self.begin(transfers, client);
                fresh = true;
            }
            let done =
                matches!(&message, ServerMessage::ActionResult { key, .. } if *key == self.key);
            said.push(message);
            if done {
                return said;
            }
        }
    }

    /// Begins and finishes on a new connection.
    async fn run(&mut self, transfers: &Transfers) -> Vec<ServerMessage> {
        let mut client = Client::new();
        self.begin(transfers, &client);
        self.finish(transfers, &mut client, true).await
    }
}

/// The attachment an upload landed as, checking the device said so both
/// ways.
fn staged(said: &[ServerMessage], key: IdempotencyKey) -> AttachmentRef {
    let done = said
        .iter()
        .find_map(|message| match message {
            ServerMessage::UploadDone { attachment, .. } => Some(attachment.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected an UploadDone in {said:?}"));
    match said.last() {
        Some(ServerMessage::ActionResult {
            key: got,
            outcome: ActionOutcome::Staged { attachment },
        }) if *got == key => {
            assert_eq!(*attachment, done);
            done
        }
        other => panic!("expected a staged result, got {other:?}"),
    }
}

/// The code a transfer failed with, checking its action was refused.
fn refused(said: &[ServerMessage]) -> ErrorCode {
    let code = said
        .iter()
        .find_map(|message| match message {
            ServerMessage::TransferFailed { code, .. } => Some(*code),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected a TransferFailed in {said:?}"));
    match said.last() {
        Some(ServerMessage::ActionResult {
            outcome: ActionOutcome::Refused { .. } | ActionOutcome::UnknownAgent,
            ..
        }) => code,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// A container whose guest runs on a folder of this device, and reads a
/// document's text by naming it.
struct Container {
    workspace: WorkspaceId,
    root: TempDir,
    files: Files,
    host: OnceLock<Transfers>,
    up: AtomicBool,
    /// What a hostile guest adds to each manifest it sends.
    adds: Mutex<Vec<ManifestEntry>>,
}

impl Container {
    fn new(room: Room, quota: u64) -> Arc<Self> {
        let root = TempDir::new();
        let files = Files::with_extractor(
            files::Config {
                attachments: root.path().join("attachments"),
                work: root.path().join("work"),
                room,
                quota,
            },
            Extractor::with("/bin/sh", &["-c", "printf 'text of %s' \"${1##*/}\"", "sh"]),
        );
        Arc::new(Self {
            workspace: WorkspaceId::generate(),
            root,
            files,
            host: OnceLock::new(),
            up: AtomicBool::new(true),
            adds: Mutex::new(Vec::new()),
        })
    }

    fn host(&self) -> &Transfers {
        self.host.get().expect("the container is linked")
    }

    fn is_up(&self) -> bool {
        self.up.load(Ordering::SeqCst)
    }

    /// Where a container path is on this device.
    fn local(&self, path: &str) -> PathBuf {
        let attachments = self.root.path().join("attachments");
        let work = self.root.path().join("work");
        swap(path, ATTACHMENTS_DIR, &attachments)
            .or_else(|| swap(path, WORK_DIR, &work))
            .unwrap_or_else(|| PathBuf::from(path))
    }

    /// What a path on this device is in the container.
    fn inside(&self, path: &str) -> String {
        let attachments = self.root.path().join("attachments");
        let attachments = attachments.to_str().unwrap();
        match path.strip_prefix(attachments) {
            Some(rest) => format!("{ATTACHMENTS_DIR}{rest}"),
            None => path.to_owned(),
        }
    }

    fn drop_link(&self) {
        self.up.store(false, Ordering::SeqCst);
        self.files.link_down();
        self.host().guest_down(self.workspace);
    }
}

fn swap(path: &str, from: &str, to: &Path) -> Option<PathBuf> {
    let rest = path.strip_prefix(from)?;
    let rest = rest.strip_prefix('/').or(rest.is_empty().then_some(""))?;
    Some(to.join(rest))
}

/// The host's end of the fake link.
struct ToGuest(Arc<Container>);

impl GuestOut for ToGuest {
    fn message(&self, message: HostToGuest) -> BoxFuture<Result<(), Closed>> {
        let container = self.0.clone();
        Box::pin(async move {
            if !container.is_up() {
                return Err(Closed);
            }
            let message = match message {
                HostToGuest::FetchBegin { transfer, path } => HostToGuest::FetchBegin {
                    transfer,
                    path: container.local(&path).to_str().unwrap().to_owned(),
                },
                HostToGuest::FetchFiles {
                    transfer,
                    cwd,
                    paths,
                } => HostToGuest::FetchFiles {
                    transfer,
                    cwd: container.local(&cwd).to_str().unwrap().to_owned(),
                    paths,
                },
                other => other,
            };
            let out = Arc::new(ToHost(container.clone()));
            container.files.message(message, out);
            Ok(())
        })
    }

    fn chunk(
        &self,
        transfer: TransferId,
        offset: u64,
        data: Vec<u8>,
    ) -> BoxFuture<Result<(), Closed>> {
        let container = self.0.clone();
        Box::pin(async move {
            if !container.is_up() {
                return Err(Closed);
            }
            container.files.chunk(transfer, offset, data);
            Ok(())
        })
    }

    fn request(
        &self,
        _wait: Duration,
        make: MakeRequest,
    ) -> BoxFuture<Result<GuestToHost, String>> {
        let container = self.0.clone();
        Box::pin(async move {
            if !container.is_up() {
                return Err("the link closed".to_owned());
            }
            match make(7) {
                HostToGuest::ListIgnored { req, cwd } => {
                    let work = container.root.path().join("work");
                    let cwd = container.local(&cwd);
                    let listed =
                        crate::guest::worktree::ignored(&work, cwd.to_str().unwrap(), None).await;
                    Ok(GuestToHost::Ignored {
                        req,
                        files: listed.files,
                        truncated: listed.truncated,
                    })
                }
                HostToGuest::Place {
                    req,
                    staged,
                    cwd,
                    overwrite,
                } => {
                    let cwd = container.local(&cwd).to_str().unwrap().to_owned();
                    let result = container.files.place(staged, cwd, overwrite).await;
                    Ok(GuestToHost::Placed { req, result })
                }
                other => Err(format!("the fake guest doesn't answer {other:?}")),
            }
        })
    }
}

/// The guest's end of the fake link.
struct ToHost(Arc<Container>);

impl Out for ToHost {
    fn message(&self, message: GuestToHost) -> BoxFuture<Result<(), Closed>> {
        let container = self.0.clone();
        Box::pin(async move {
            if !container.is_up() {
                return Err(Closed);
            }
            let message = match message {
                GuestToHost::UploadStored {
                    transfer,
                    paths,
                    thumbnail,
                    text,
                } => GuestToHost::UploadStored {
                    transfer,
                    paths: paths.iter().map(|path| container.inside(path)).collect(),
                    thumbnail,
                    text: text.map(|text| container.inside(&text)),
                },
                GuestToHost::FetchManifest {
                    transfer,
                    mut entries,
                } => {
                    entries.extend(container.adds.lock().unwrap().iter().cloned());
                    GuestToHost::FetchManifest { transfer, entries }
                }
                other => other,
            };
            container.host().guest_message(container.workspace, message);
            Ok(())
        })
    }

    fn chunk(
        &self,
        transfer: TransferId,
        offset: u64,
        data: Vec<u8>,
    ) -> BoxFuture<Result<(), Closed>> {
        let container = self.0.clone();
        Box::pin(async move {
            if !container.is_up() {
                return Err(Closed);
            }
            container
                .host()
                .guest_chunk(container.workspace, transfer, offset, data);
            Ok(())
        })
    }
}

struct Linked(Arc<Container>);

impl Guests for Linked {
    fn link(&self, workspace: WorkspaceId) -> Option<Arc<dyn GuestOut>> {
        (workspace == self.0.workspace && self.0.is_up())
            .then(|| Arc::new(ToGuest(self.0.clone())) as Arc<dyn GuestOut>)
    }
}

/// One agent on this device and one in the container.
struct Map {
    local: Place,
    boxed: Place,
    local_agent: AgentId,
    boxed_agent: AgentId,
}

impl Places for Map {
    fn agent(&self, agent: AgentId) -> Option<Place> {
        if agent == self.local_agent {
            Some(self.local.clone())
        } else if agent == self.boxed_agent {
            Some(self.boxed.clone())
        } else {
            None
        }
    }

    fn workspace(&self, workspace: WorkspaceId) -> Option<(PlaceHost, PathBuf)> {
        if workspace == self.local.workspace {
            Some((PlaceHost::Local, self.local.cwd.clone()))
        } else if workspace == self.boxed.workspace {
            Some((self.boxed.host, self.boxed.cwd.clone()))
        } else {
            None
        }
    }
}

/// This device's transfers, linked to one container.
struct Rig {
    transfers: Transfers,
    device: TempDir,
    container: Arc<Container>,
    local: Place,
    boxed: Place,
    local_agent: AgentId,
    boxed_agent: AgentId,
}

impl Rig {
    fn new() -> Self {
        Self::with(ANY_ROOM, DEVICE_QUOTA)
    }

    /// A rig whose device and container both have these limits.
    fn with(room: Room, quota: u64) -> Self {
        Self::build(room, quota, true)
    }

    /// A rig whose device keeps no skill store.
    fn without_skills() -> Self {
        Self::build(ANY_ROOM, DEVICE_QUOTA, false)
    }

    fn build(room: Room, quota: u64, skills: bool) -> Self {
        let device = TempDir::new();
        let container = Container::new(room, quota);
        let (local_agent, boxed_agent) = (AgentId::generate(), AgentId::generate());
        let work = device.path().join("work");
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(container.root.path().join("work/repo")).unwrap();
        let local = Place {
            device: DeviceId::generate(),
            workspace: WorkspaceId::generate(),
            cwd: work,
            attachments: device
                .path()
                .join("attachments")
                .join(local_agent.to_string()),
            host: PlaceHost::Local,
            dispatcher: false,
            trust: None,
            claude_artifacts: true,
        };
        let boxed = Place {
            device: local.device,
            workspace: container.workspace,
            cwd: PathBuf::from(format!("{WORK_DIR}/repo")),
            attachments: Path::new(ATTACHMENTS_DIR).join(boxed_agent.to_string()),
            host: PlaceHost::Guest {
                workspace: container.workspace,
            },
            dispatcher: false,
            trust: None,
            claude_artifacts: true,
        };
        let transfers = Transfers::new(Config {
            attachments: device.path().join("attachments"),
            room,
            quota,
            places: Arc::new(Map {
                local: local.clone(),
                boxed: boxed.clone(),
                local_agent,
                boxed_agent,
            }),
            guests: Arc::new(Linked(container.clone())),
            skills: skills.then(|| {
                Arc::new(FolderStore::new(device.path().join("skills"))) as Arc<dyn TreeStore>
            }),
        });
        let _ = container.host.set(transfers.clone());
        Self {
            transfers,
            device,
            container,
            local,
            boxed,
            local_agent,
            boxed_agent,
        }
    }

    fn attachments(&self) -> PathBuf {
        self.device.path().join("attachments")
    }

    /// Downloads `path` from an agent's folders, granting credit for all
    /// of it at once. Gives the manifest and the bytes, or the code the
    /// download failed with.
    async fn fetch(
        &self,
        agent: AgentId,
        path: &str,
    ) -> Result<(Vec<ManifestEntry>, Vec<u8>), ErrorCode> {
        self.download(|route, key, transfer| {
            self.transfers
                .fetch_begin(route, key, transfer, agent, path.to_owned(), false);
        })
        .await
    }

    /// Downloads `path` as `fetch` does, for a client that speaks this
    /// build's protocol, with Show anyway if `anyway`.
    async fn fetch_now(
        &self,
        agent: AgentId,
        path: &str,
        anyway: bool,
    ) -> Result<(Vec<ManifestEntry>, Vec<u8>), ErrorCode> {
        self.download_speaking(Some(PROTOCOL_VERSION), |route, key, transfer| {
            self.transfers
                .fetch_begin(route, key, transfer, agent, path.to_owned(), anyway);
        })
        .await
    }

    /// Downloads chosen files from a workspace's checkout, as `fetch` does.
    async fn fetch_files(
        &self,
        workspace: WorkspaceId,
        paths: &[&str],
    ) -> Result<(Vec<ManifestEntry>, Vec<u8>), ErrorCode> {
        let paths = paths.iter().map(|&path| path.to_owned()).collect();
        self.download(|route, key, transfer| {
            self.transfers
                .fetch_files(route, key, transfer, workspace, paths);
        })
        .await
    }

    /// Downloads a tree from this device's skill store, as `fetch` does.
    async fn fetch_tree(
        &self,
        tree: &TreeHash,
    ) -> Result<(Vec<ManifestEntry>, Vec<u8>), ErrorCode> {
        self.download(|route, key, transfer| {
            self.transfers
                .fetch_tree(route, key, transfer, tree.clone());
        })
        .await
    }

    /// Runs a download that `begin` starts.
    async fn download(
        &self,
        begin: impl FnOnce(&Route, IdempotencyKey, TransferId),
    ) -> Result<(Vec<ManifestEntry>, Vec<u8>), ErrorCode> {
        self.download_speaking(None, begin).await
    }

    /// Runs a download that `begin` starts, for a client that agreed
    /// `protocol` if it said hello.
    async fn download_speaking(
        &self,
        protocol: Option<u32>,
        begin: impl FnOnce(&Route, IdempotencyKey, TransferId),
    ) -> Result<(Vec<ManifestEntry>, Vec<u8>), ErrorCode> {
        let mut client = Client::new();
        if let Some(protocol) = protocol {
            client.route = client.route.speaking(protocol);
        }
        let key = IdempotencyKey::generate();
        let transfer = TransferId::generate();
        begin(&client.route, key, transfer);
        let entries = match client.message().await {
            ServerMessage::FetchManifest { entries, .. } => entries,
            ServerMessage::TransferFailed { code, .. } => {
                assert!(matches!(
                    client.message().await,
                    ServerMessage::ActionResult {
                        outcome: ActionOutcome::Refused { .. } | ActionOutcome::UnknownAgent,
                        ..
                    }
                ));
                return Err(code);
            }
            other => panic!("expected a manifest, got {other:?}"),
        };
        let total = Layout::of(&entries).total();
        let mut bytes = vec![0; usize::try_from(total).unwrap()];
        let mut have = 0;
        self.transfers.credit(transfer, 0, total);
        while have < total {
            match client.next().await {
                Got::Chunk {
                    transfer: got,
                    offset,
                    data,
                } if got == transfer => {
                    let at = usize::try_from(offset).unwrap();
                    bytes[at..at + data.len()].copy_from_slice(&data);
                    have += data.len() as u64;
                }
                other => panic!("expected a chunk, got {other:?}"),
            }
        }
        self.transfers.credit(transfer, total, total);
        match client.message().await {
            ServerMessage::ActionResult {
                key: got,
                outcome: ActionOutcome::Done,
            } if got == key => Ok((entries, bytes)),
            other => panic!("expected the download done, got {other:?}"),
        }
    }
}

/// A folder to upload from, removed with the test.
fn source() -> TempDir {
    TempDir::new()
}

/// A small PNG.
fn picture() -> Vec<u8> {
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(40, 30, image::Rgb([200, 10, 10]))
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("a picture");
    png.into_inner()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[tokio::test]
async fn an_upload_to_the_device_lands_in_staging() {
    let rig = Rig::new();
    let src = source();
    let folder = src.path().join("notes");
    fs::create_dir_all(folder.join("sub")).unwrap();
    fs::write(folder.join("a.txt"), b"hello").unwrap();
    fs::write(folder.join("sub/run.sh"), b"#!/bin/sh\n").unwrap();
    fs::set_permissions(folder.join("sub/run.sh"), Permissions::from_mode(0o700)).unwrap();
    fs::write(folder.join(".DS_Store"), b"junk").unwrap();

    let mut upload = Upload::new(UploadTarget::Device, &folder);
    let attachment = staged(&upload.run(&rig.transfers).await, upload.key);

    assert_eq!(attachment.id, AttachmentId(upload.transfer.0));
    assert_eq!(attachment.kind, AttachmentKind::Folder);
    let landed = rig
        .attachments()
        .join("staged")
        .join(upload.transfer.to_string())
        .join("notes");
    assert_eq!(attachment.path, landed.to_str().unwrap());
    assert_eq!(fs::read(landed.join("a.txt")).unwrap(), b"hello");
    assert_eq!(mode(&landed.join("a.txt")), 0o644);
    assert_eq!(mode(&landed.join("sub/run.sh")), 0o755);
    assert!(!landed.join(".DS_Store").exists());
    assert!(!rig
        .attachments()
        .join("incoming")
        .join(upload.transfer.to_string())
        .exists());

    // A begin sent again once it landed gets the same answer.
    let again = upload.run(&rig.transfers).await;
    assert_eq!(staged(&again, upload.key), attachment);
}

#[tokio::test]
async fn an_upload_resumes_from_what_landed_after_the_link_drops() {
    let rig = Rig::new();
    let src = source();
    let data = pattern(3 * 1024 * 1024 + 7);
    let path = src.path().join("big.bin");
    fs::write(&path, &data).unwrap();
    let mut upload = Upload::new(UploadTarget::Device, &path);

    let mut first = Client::new();
    upload.begin(&rig.transfers, &first);
    let mut fresh = true;
    let half = 1024 * 1024;
    loop {
        let message = first.message().await;
        match upload.credit(&message, &mut fresh) {
            Some(committed) if committed >= half => break,
            Some(_) => upload.send(&rig.transfers, half),
            None => panic!("expected credit, got {message:?}"),
        }
    }
    drop(first);

    let mut second = Client::new();
    upload.begin(&rig.transfers, &second);
    let said = upload.finish(&rig.transfers, &mut second, true).await;
    let resumed = said.iter().find_map(|message| match message {
        ServerMessage::TransferCredit { committed, .. } => Some(*committed),
        _ => None,
    });
    assert_eq!(resumed, Some(half));
    let attachment = staged(&said, upload.key);
    assert_eq!(fs::read(&attachment.path).unwrap(), data);
}

#[tokio::test]
async fn a_file_whose_hash_doesnt_match_is_sent_again() {
    let rig = Rig::new();
    let src = source();
    let data = pattern(700 * 1024);
    let path = src.path().join("photo.png");
    fs::write(&path, &data).unwrap();
    let mut upload = Upload::new(UploadTarget::Device, &path);
    upload.corrupt = true;

    let said = upload.run(&rig.transfers).await;
    assert!(said.iter().any(|message| matches!(
        message,
        ServerMessage::TransferFailed {
            code: ErrorCode::HashMismatch,
            ..
        }
    )));
    let attachment = staged(&said, upload.key);
    assert_eq!(attachment.kind, AttachmentKind::Image);
    assert_eq!(fs::read(&attachment.path).unwrap(), data);
}

#[tokio::test]
async fn an_upload_past_the_quota_is_refused() {
    let rig = Rig::with(ANY_ROOM, 100);
    let src = source();
    let path = src.path().join("big.bin");
    fs::write(&path, pattern(1000)).unwrap();
    let mut upload = Upload::new(UploadTarget::Device, &path);
    assert_eq!(
        refused(&upload.run(&rig.transfers).await),
        ErrorCode::QuotaExceeded
    );
    assert!(!rig
        .attachments()
        .join("staged")
        .join(upload.transfer.to_string())
        .exists());
}

#[tokio::test]
async fn an_upload_that_would_fill_the_disk_is_refused() {
    let rig = Rig::with(
        Room {
            keep_bytes: u64::MAX / 2,
            keep_percent: 0,
            keep_at_most: 0,
        },
        DEVICE_QUOTA,
    );
    let src = source();
    let path = src.path().join("big.bin");
    fs::write(&path, pattern(1000)).unwrap();
    let mut upload = Upload::new(UploadTarget::Device, &path);
    assert_eq!(
        refused(&upload.run(&rig.transfers).await),
        ErrorCode::NoSpace
    );

    // The container checks its own disk.
    let mut upload = Upload::new(
        UploadTarget::Workspace {
            workspace: rig.container.workspace,
        },
        &path,
    );
    assert_eq!(
        refused(&upload.run(&rig.transfers).await),
        ErrorCode::NoSpace
    );
}

#[tokio::test]
async fn an_upload_whose_paths_escape_is_refused() {
    let rig = Rig::new();
    let src = source();
    let path = src.path().join("a.txt");
    fs::write(&path, b"data").unwrap();
    let walk = maxhq_transfer::walk(&path).unwrap();
    for bad in ["../a.txt", "/etc/a.txt", "x/../../a.txt"] {
        for target in [
            UploadTarget::Device,
            UploadTarget::Agent {
                agent: rig.boxed_agent,
            },
        ] {
            let mut entries = walk.entries.clone();
            entries[0].path = bad.to_owned();
            let mut client = Client::new();
            rig.transfers.upload_begin(
                &client.route,
                IdempotencyKey::generate(),
                TransferId::generate(),
                target,
                UploadSource::Stream {
                    name: "a.txt".into(),
                    entries,
                },
            );
            let said = vec![client.message().await, client.message().await];
            assert_eq!(refused(&said), ErrorCode::BadPath, "{bad}");
        }
    }
    assert!(!src.path().parent().unwrap().join("a.txt").exists());
}

#[tokio::test]
async fn an_upload_for_an_unknown_agent_is_refused() {
    let rig = Rig::new();
    let src = source();
    let path = src.path().join("a.txt");
    fs::write(&path, b"data").unwrap();
    let mut upload = Upload::new(
        UploadTarget::Agent {
            agent: AgentId::generate(),
        },
        &path,
    );
    let said = upload.run(&rig.transfers).await;
    assert_eq!(refused(&said), ErrorCode::NoSuchAgent);
    assert!(matches!(
        said.last(),
        Some(ServerMessage::ActionResult {
            outcome: ActionOutcome::UnknownAgent,
            ..
        })
    ));
}

#[tokio::test]
async fn an_upload_to_the_skill_store_is_refused_until_it_keeps_one() {
    let rig = Rig::without_skills();
    let src = source();
    let path = src.path().join("a.txt");
    fs::write(&path, b"data").unwrap();
    let mut upload = Upload::new(
        UploadTarget::SkillStore {
            tree: TreeHash("ab12".into()),
        },
        &path,
    );
    let said = upload.run(&rig.transfers).await;
    assert_eq!(refused(&said), ErrorCode::NotAllowed);
    assert!(matches!(
        said.last(),
        Some(ServerMessage::ActionResult {
            outcome: ActionOutcome::Refused { reason },
            ..
        }) if reason == crate::skills::NOT_YET
    ));
}

/// A skill folder with a file large enough to send in parts.
fn skill_folder(src: &TempDir) -> (PathBuf, TreeHash) {
    let folder = src.path().join("big-skill");
    fs::create_dir_all(folder.join("scripts")).unwrap();
    fs::write(folder.join("SKILL.md"), "---\nname: big-skill\n---\n").unwrap();
    fs::write(folder.join("scripts/data.bin"), pattern(3 * 1024 * 1024)).unwrap();
    let tree = maxhq_core::skills::tree::of(&DiskFs, &folder).unwrap();
    (folder, tree)
}

fn done(said: &[ServerMessage], key: IdempotencyKey) {
    assert!(
        matches!(
            said.last(),
            Some(ServerMessage::ActionResult { key: got, outcome: ActionOutcome::Done }) if *got == key
        ),
        "expected done, got {said:?}"
    );
    assert!(!said
        .iter()
        .any(|message| matches!(message, ServerMessage::UploadDone { .. })));
}

#[tokio::test]
async fn a_resumed_skill_upload_lands_in_the_store_once() {
    let rig = Rig::new();
    let src = source();
    let (folder, tree) = skill_folder(&src);
    let store = FolderStore::new(rig.device.path().join("skills"));
    let mut upload = Upload::new(UploadTarget::SkillStore { tree: tree.clone() }, &folder);

    let mut first = Client::new();
    upload.begin(&rig.transfers, &first);
    let mut fresh = true;
    let half = 1024 * 1024;
    loop {
        let message = first.message().await;
        match upload.credit(&message, &mut fresh) {
            Some(committed) if committed >= half => break,
            Some(_) => upload.send(&rig.transfers, half),
            None => panic!("expected credit, got {message:?}"),
        }
    }
    drop(first);
    assert!(!store.has(&tree));

    let mut second = Client::new();
    upload.begin(&rig.transfers, &second);
    let said = upload.finish(&rig.transfers, &mut second, true).await;
    done(&said, upload.key);
    assert!(store.has(&tree));
    let landed = store.path(&tree).unwrap();
    assert_eq!(
        maxhq_core::skills::tree::of(&DiskFs, &landed).unwrap(),
        tree
    );
    assert!(!rig
        .device
        .path()
        .join("attachments/incoming")
        .join(upload.transfer.to_string())
        .exists());

    // Sent again, or sent by another upload, it's there already.
    done(&upload.run(&rig.transfers).await, upload.key);
    let mut other = Upload::new(UploadTarget::SkillStore { tree: tree.clone() }, &folder);
    let said = other.run(&rig.transfers).await;
    done(&said, other.key);
    assert!(!said
        .iter()
        .any(|message| matches!(message, ServerMessage::TransferCredit { .. })));
}

#[tokio::test]
async fn a_skill_upload_that_doesnt_match_its_tree_is_refused() {
    let rig = Rig::new();
    let src = source();
    let (folder, _) = skill_folder(&src);
    let wrong = TreeHash("a".repeat(64));
    let mut upload = Upload::new(
        UploadTarget::SkillStore {
            tree: wrong.clone(),
        },
        &folder,
    );
    let said = upload.run(&rig.transfers).await;
    assert_eq!(refused(&said), ErrorCode::NotAllowed);
    let store = FolderStore::new(rig.device.path().join("skills"));
    assert!(!store.has(&wrong));
    assert!(!rig
        .device
        .path()
        .join("attachments/incoming")
        .join(upload.transfer.to_string())
        .exists());

    let mut bad = Upload::new(
        UploadTarget::SkillStore {
            tree: TreeHash("../x".into()),
        },
        &folder,
    );
    assert_eq!(refused(&bad.run(&rig.transfers).await), ErrorCode::BadPath);
}

#[tokio::test]
async fn a_tree_in_the_store_downloads_and_one_missing_is_refused() {
    let rig = Rig::new();
    let src = source();
    let (folder, tree) = skill_folder(&src);
    let mut upload = Upload::new(UploadTarget::SkillStore { tree: tree.clone() }, &folder);
    done(&upload.run(&rig.transfers).await, upload.key);

    let (entries, bytes) = rig.fetch_tree(&tree).await.unwrap();
    let paths: Vec<String> = entries.iter().map(|entry| entry.path.clone()).collect();
    let top = &tree.0;
    assert_eq!(
        paths,
        [
            top.clone(),
            format!("{top}/SKILL.md"),
            format!("{top}/scripts"),
            format!("{top}/scripts/data.bin"),
        ]
    );
    assert_eq!(
        bytes.len(),
        3 * 1024 * 1024 + "---\nname: big-skill\n---\n".len()
    );

    let missing = TreeHash("b".repeat(64));
    assert_eq!(rig.fetch_tree(&missing).await, Err(ErrorCode::BadPath));
    let bad = TreeHash("../x".into());
    assert_eq!(rig.fetch_tree(&bad).await, Err(ErrorCode::BadPath));
    let none = Rig::without_skills();
    assert_eq!(none.fetch_tree(&tree).await, Err(ErrorCode::NotAllowed));
}

#[tokio::test]
async fn a_cancelled_upload_leaves_nothing() {
    let rig = Rig::new();
    let src = source();
    let path = src.path().join("big.bin");
    fs::write(&path, pattern(2 * 1024 * 1024)).unwrap();
    let mut upload = Upload::new(UploadTarget::Device, &path);
    let mut client = Client::new();
    upload.begin(&rig.transfers, &client);
    let mut fresh = true;
    let message = client.message().await;
    upload.credit(&message, &mut fresh).expect("credit");
    upload.send(&rig.transfers, 256 * 1024);
    rig.transfers.cancel(upload.transfer);
    let said = upload.finish(&rig.transfers, &mut client, false).await;
    assert!(matches!(
        said.last(),
        Some(ServerMessage::ActionResult {
            outcome: ActionOutcome::Refused { .. },
            ..
        })
    ));
    let incoming = rig
        .attachments()
        .join("incoming")
        .join(upload.transfer.to_string());
    assert!(!incoming.exists());
}

#[tokio::test]
async fn a_download_from_the_device_comes_in_chunks() {
    let rig = Rig::new();
    let data = pattern(1536 * 1024 + 3);
    fs::create_dir_all(rig.local.cwd.join("out")).unwrap();
    fs::write(rig.local.cwd.join("out/report.bin"), &data).unwrap();

    let (entries, bytes) = rig.fetch(rig.local_agent, "out/report.bin").await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "report.bin");
    assert_eq!(bytes, data);

    // An absolute path inside the agent's folder works too.
    let absolute = rig.local.cwd.join("out/report.bin");
    let (_, bytes) = rig
        .fetch(rig.local_agent, absolute.to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(bytes, data);
}

#[tokio::test]
async fn a_download_never_follows_a_link_or_leaves_the_agents_folders() {
    let rig = Rig::new();
    let outside = TempDir::new();
    fs::write(outside.path().join("secret"), b"secret").unwrap();
    let cwd = &rig.local.cwd;
    symlink(outside.path().join("secret"), cwd.join("link")).unwrap();
    symlink(outside.path(), cwd.join("away")).unwrap();
    fs::create_dir_all(cwd.join("tree")).unwrap();
    fs::write(cwd.join("tree/kept.txt"), b"kept").unwrap();
    symlink(outside.path().join("secret"), cwd.join("tree/link")).unwrap();
    symlink("kept.txt", cwd.join("tree/near")).unwrap();

    for bad in [
        "link",
        "away/secret",
        "../secret",
        "/etc/hosts",
        outside.path().join("secret").to_str().unwrap(),
    ] {
        assert_eq!(
            rig.fetch(rig.local_agent, bad).await.unwrap_err(),
            ErrorCode::BadPath,
            "{bad}"
        );
    }

    // In a folder, a link that stays inside comes as a link, and one that
    // points outside doesn't come at all.
    let (entries, bytes) = rig.fetch(rig.local_agent, "tree").await.unwrap();
    let near = entries
        .iter()
        .find(|entry| entry.path == "tree/near")
        .unwrap();
    assert!(matches!(&near.kind, EntryKind::Symlink { target } if target == "kept.txt"));
    assert!(!entries.iter().any(|entry| entry.path == "tree/link"));
    assert_eq!(bytes, b"kept");
}

#[tokio::test]
async fn show_anyway_reads_one_named_file_outside_the_agents_folders() {
    let rig = Rig::new();
    let outside = TempDir::new();
    let dir = outside.path();
    fs::write(dir.join("plan.md"), b"# Plan").unwrap();
    let plan = dir.join("plan.md");
    let plan = plan.to_str().unwrap();

    // Without it the path is refused as outside, and as a bad path to a
    // client from before Show anyway.
    assert_eq!(
        rig.fetch_now(rig.local_agent, plan, false)
            .await
            .unwrap_err(),
        ErrorCode::Outside
    );
    assert_eq!(
        rig.fetch(rig.local_agent, plan).await.unwrap_err(),
        ErrorCode::BadPath
    );
    let older = rig
        .download_speaking(Some(SHOW_ANYWAY_SINCE - 1), |route, key, transfer| {
            rig.transfers.fetch_begin(
                route,
                key,
                transfer,
                rig.local_agent,
                plan.to_owned(),
                false,
            );
        })
        .await;
    assert_eq!(older.unwrap_err(), ErrorCode::BadPath);

    let (entries, bytes) = rig.fetch_now(rig.local_agent, plan, true).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(bytes, b"# Plan");

    // A link is followed to the file it names.
    symlink(dir.join("plan.md"), dir.join("link.md")).unwrap();
    let link = dir.join("link.md");
    let (_, bytes) = rig
        .fetch_now(rig.local_agent, link.to_str().unwrap(), true)
        .await
        .unwrap();
    assert_eq!(bytes, b"# Plan");

    // Code the viewer shows as text comes the same way.
    fs::write(dir.join("main.rs"), b"fn main() {}").unwrap();
    let (_, bytes) = rig
        .fetch_now(rig.local_agent, dir.join("main.rs").to_str().unwrap(), true)
        .await
        .unwrap();
    assert_eq!(bytes, b"fn main() {}");
}

#[tokio::test]
async fn show_anyway_still_wants_one_file_the_viewer_shows() {
    let rig = Rig::new();
    let outside = TempDir::new();
    let dir = outside.path();
    fs::write(dir.join("secret.bin"), b"secret").unwrap();
    fs::write(dir.join(".env"), b"TOKEN=secret").unwrap();
    fs::write(dir.join("server.pem"), b"secret").unwrap();
    fs::create_dir_all(dir.join("notes.md")).unwrap();
    symlink(dir.join("secret.bin"), dir.join("shot.png")).unwrap();
    let at = |name: &str| dir.join(name).to_str().unwrap().to_owned();

    for (path, code) in [
        ("plan.md".to_owned(), ErrorCode::BadPath),
        (at("secret.bin"), ErrorCode::NotAllowed),
        (at(".env"), ErrorCode::NotAllowed),
        (at("server.pem"), ErrorCode::NotAllowed),
        (at("shot.png"), ErrorCode::NotAllowed),
        (at("notes.md"), ErrorCode::BadPath),
        (at("gone.md"), ErrorCode::BadPath),
    ] {
        assert_eq!(
            rig.fetch_now(rig.local_agent, &path, true)
                .await
                .unwrap_err(),
            code,
            "{path}"
        );
    }

    // An agent in a container isn't reached yet.
    fs::write(dir.join("plan.md"), b"# Plan").unwrap();
    assert_eq!(
        rig.fetch_now(rig.boxed_agent, &at("plan.md"), true)
            .await
            .unwrap_err(),
        ErrorCode::NotAllowed
    );
}

#[tokio::test]
async fn an_upload_to_a_workspace_lands_in_the_containers_staging() {
    let rig = Rig::new();
    let src = source();
    let data = pattern(1536 * 1024);
    let path = src.path().join("data.bin");
    fs::write(&path, &data).unwrap();
    let mut upload = Upload::new(
        UploadTarget::Workspace {
            workspace: rig.container.workspace,
        },
        &path,
    );
    let attachment = staged(&upload.run(&rig.transfers).await, upload.key);
    let inside = format!("{ATTACHMENTS_DIR}/staged/{}/data.bin", upload.transfer);
    assert_eq!(attachment.path, inside);
    assert_eq!(fs::read(rig.container.local(&inside)).unwrap(), data);
    assert!(!rig
        .attachments()
        .join("staged")
        .join(upload.transfer.to_string())
        .exists());

    // A begin sent again once it landed gets the same answer, from the
    // guest's disk.
    let again = upload.run(&rig.transfers).await;
    assert_eq!(staged(&again, upload.key), attachment);
}

#[tokio::test]
async fn an_upload_to_an_agent_in_a_container_lands_in_its_folder() {
    let rig = Rig::new();
    let src = source();
    let folder = src.path().join("pics");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("one.png"), pattern(300 * 1024)).unwrap();
    fs::write(folder.join("two.png"), pattern(900 * 1024)).unwrap();
    let mut upload = Upload::new(
        UploadTarget::Agent {
            agent: rig.boxed_agent,
        },
        &folder,
    );
    upload.corrupt = true;
    let said = upload.run(&rig.transfers).await;
    assert!(said.iter().any(|message| matches!(
        message,
        ServerMessage::TransferFailed {
            code: ErrorCode::HashMismatch,
            ..
        }
    )));
    let attachment = staged(&said, upload.key);
    let inside = format!(
        "{ATTACHMENTS_DIR}/{}/{}/pics",
        rig.boxed_agent, upload.transfer
    );
    assert_eq!(attachment.path, inside);
    let landed = rig.container.local(&inside);
    assert_eq!(
        fs::read(landed.join("two.png")).unwrap(),
        pattern(900 * 1024)
    );
    assert_eq!(
        fs::read(landed.join("one.png")).unwrap(),
        pattern(300 * 1024)
    );
}

#[tokio::test]
async fn a_relayed_upload_resumes_after_the_clients_link_drops() {
    let rig = Rig::new();
    let src = source();
    let data = pattern(3 * 1024 * 1024);
    let path = src.path().join("big.bin");
    fs::write(&path, &data).unwrap();
    let mut upload = Upload::new(
        UploadTarget::Agent {
            agent: rig.boxed_agent,
        },
        &path,
    );
    let mut first = Client::new();
    upload.begin(&rig.transfers, &first);
    let mut fresh = true;
    let half = 1024 * 1024;
    loop {
        let message = first.message().await;
        match upload.credit(&message, &mut fresh) {
            Some(committed) if committed >= half => break,
            Some(_) => upload.send(&rig.transfers, half),
            None => panic!("expected credit, got {message:?}"),
        }
    }
    drop(first);

    let mut second = Client::new();
    upload.begin(&rig.transfers, &second);
    let said = upload.finish(&rig.transfers, &mut second, true).await;
    let resumed = said.iter().find_map(|message| match message {
        ServerMessage::TransferCredit { committed, .. } => Some(*committed),
        _ => None,
    });
    assert_eq!(resumed, Some(half));
    let attachment = staged(&said, upload.key);
    assert_eq!(
        fs::read(rig.container.local(&attachment.path)).unwrap(),
        data
    );
}

#[tokio::test]
async fn a_relayed_upload_fails_when_the_guest_link_drops() {
    let rig = Rig::new();
    let src = source();
    let path = src.path().join("big.bin");
    fs::write(&path, pattern(2 * 1024 * 1024)).unwrap();
    let mut upload = Upload::new(
        UploadTarget::Workspace {
            workspace: rig.container.workspace,
        },
        &path,
    );
    let mut client = Client::new();
    upload.begin(&rig.transfers, &client);
    let mut fresh = true;
    let message = client.message().await;
    upload.credit(&message, &mut fresh).expect("credit");
    rig.container.drop_link();
    let said = upload.finish(&rig.transfers, &mut client, false).await;
    assert_eq!(refused(&said), ErrorCode::Unreachable);

    // With the link down, a new upload is refused at once.
    let mut again = Upload::new(
        UploadTarget::Workspace {
            workspace: rig.container.workspace,
        },
        &path,
    );
    assert_eq!(
        refused(&again.run(&rig.transfers).await),
        ErrorCode::Unreachable
    );
}

#[tokio::test]
async fn a_download_from_a_container_relays_through_its_guest() {
    let rig = Rig::new();
    let data = pattern(1200 * 1024);
    let repo = rig.container.root.path().join("work/repo");
    fs::write(repo.join("out.bin"), &data).unwrap();
    symlink("/etc/hosts", repo.join("link")).unwrap();

    let (entries, bytes) = rig.fetch(rig.boxed_agent, "out.bin").await.unwrap();
    assert_eq!(entries[0].path, "out.bin");
    assert_eq!(bytes, data);

    for bad in ["link", "../../../etc/hosts", "/etc/hosts"] {
        assert_eq!(
            rig.fetch(rig.boxed_agent, bad).await.unwrap_err(),
            ErrorCode::BadPath,
            "{bad}"
        );
    }
}

#[tokio::test]
async fn what_the_path_search_found_in_a_container_stages_on_the_device() {
    let rig = Rig::new();
    let repo = rig.container.root.path().join("work/repo");
    fs::create_dir_all(repo.join("src/deep")).unwrap();
    fs::write(repo.join("src/main.rs"), b"fn main() {}").unwrap();
    fs::write(repo.join("src/deep/lib.rs"), b"pub fn lib() {}").unwrap();
    symlink("/etc/hosts", repo.join("link")).unwrap();

    let id = AttachmentId::generate();
    let file = rig
        .transfers
        .stage_found(&rig.boxed, id, "src/main.rs")
        .await
        .unwrap();
    assert_eq!((file.id, file.name.as_str()), (id, "main.rs"));
    assert_eq!(fs::read(&file.path).unwrap(), b"fn main() {}");
    assert!(file.path.starts_with(rig.attachments().to_str().unwrap()));

    let folder = rig
        .transfers
        .stage_found(&rig.boxed, AttachmentId::generate(), "src")
        .await
        .unwrap();
    assert_eq!(folder.kind, AttachmentKind::Folder);
    let lib = Path::new(&folder.path).join("deep/lib.rs");
    assert_eq!(fs::read(lib).unwrap(), b"pub fn lib() {}");

    for bad in ["link", "../../../etc/hosts", "/etc/hosts"] {
        let id = AttachmentId::generate();
        let err = rig.transfers.stage_found(&rig.boxed, id, bad).await;
        assert!(err.unwrap_err().starts_with(bad), "{bad}");
    }
}

/// Stages a file on this device as an upload would, and gives its ID.
async fn stage_on_device(rig: &Rig, name: &str, data: &[u8]) -> AttachmentRef {
    let src = source();
    let path = src.path().join(name);
    fs::write(&path, data).unwrap();
    let mut upload = Upload::new(UploadTarget::Device, &path);
    staged(&upload.run(&rig.transfers).await, upload.key)
}

#[tokio::test]
async fn delivering_to_an_agent_on_the_device_moves_the_staged_copy() {
    let rig = Rig::new();
    let attachment = stage_on_device(&rig, "a.txt", b"hello").await;
    let delivered = rig
        .transfers
        .deliver(&rig.local, vec![attachment.id])
        .await
        .unwrap();
    let want = rig
        .local
        .attachments
        .join(attachment.id.to_string())
        .join("a.txt");
    assert_eq!(delivered[0].path, want);
    assert_eq!(fs::read(&want).unwrap(), b"hello");

    // A message to the same agent finds it where it went, such as a
    // queued one taken back and sent again.
    let again = rig
        .transfers
        .deliver(&rig.local, vec![attachment.id])
        .await
        .unwrap();
    assert_eq!(again, delivered);
    assert_eq!(fs::read(&want).unwrap(), b"hello");

    // Another agent's message can't name it.
    let other = Place {
        attachments: rig.attachments().join(AgentId::generate().to_string()),
        ..rig.local.clone()
    };
    assert!(matches!(
        rig.transfers.deliver(&other, vec![attachment.id]).await,
        Err(DeliverError::Missing(id)) if id == attachment.id
    ));
    assert!(want.exists());
}

#[tokio::test]
async fn delivering_into_a_container_pushes_the_staged_copy() {
    let rig = Rig::new();
    let data = pattern(1100 * 1024);
    let attachment = stage_on_device(&rig, "doc.pdf", &data).await;
    let delivered = rig
        .transfers
        .deliver(&rig.boxed, vec![attachment.id])
        .await
        .unwrap();
    let [moved] = delivered.as_slice() else {
        panic!("expected one delivery");
    };
    assert_eq!(moved.attachment.id, attachment.id);
    assert_eq!(moved.attachment.name, "doc.pdf");
    assert_eq!(moved.attachment.kind, AttachmentKind::Pdf);
    let inside = moved.path.to_str().unwrap();
    let prefix = format!("{ATTACHMENTS_DIR}/{}/", rig.boxed_agent);
    assert!(inside.starts_with(&prefix), "{inside}");
    assert!(inside.ends_with("/doc.pdf"), "{inside}");
    assert_eq!(fs::read(rig.container.local(inside)).unwrap(), data);
    assert!(!rig
        .attachments()
        .join("staged")
        .join(attachment.id.to_string())
        .exists());

    // A message to the same agent finds it where it went.
    let again = rig
        .transfers
        .deliver(&rig.boxed, vec![attachment.id])
        .await
        .unwrap();
    assert_eq!(again, delivered);

    // Another agent's message in the same container can't name it.
    let other = Place {
        attachments: Path::new(ATTACHMENTS_DIR).join(AgentId::generate().to_string()),
        ..rig.boxed.clone()
    };
    assert!(matches!(
        rig.transfers.deliver(&other, vec![attachment.id]).await,
        Err(DeliverError::Missing(id)) if id == attachment.id
    ));
    assert_eq!(fs::read(rig.container.local(inside)).unwrap(), data);
}

#[tokio::test]
async fn delivering_moves_an_upload_out_of_the_containers_staging() {
    let rig = Rig::new();
    let src = source();
    let path = src.path().join("notes.txt");
    fs::write(&path, b"notes").unwrap();
    let mut upload = Upload::new(
        UploadTarget::Workspace {
            workspace: rig.container.workspace,
        },
        &path,
    );
    let attachment = staged(&upload.run(&rig.transfers).await, upload.key);

    let delivered = rig
        .transfers
        .deliver(&rig.boxed, vec![attachment.id])
        .await
        .unwrap();
    let inside = format!(
        "{ATTACHMENTS_DIR}/{}/{}/notes.txt",
        rig.boxed_agent, upload.transfer
    );
    assert_eq!(delivered[0].path, PathBuf::from(&inside));
    assert_eq!(delivered[0].attachment.id, attachment.id);
    assert_eq!(fs::read(rig.container.local(&inside)).unwrap(), b"notes");
    assert!(!rig.container.local(&attachment.path).exists());

    // Delivering it again finds it where it went.
    let again = rig
        .transfers
        .deliver(&rig.boxed, vec![attachment.id])
        .await
        .unwrap();
    assert_eq!(again[0].path, PathBuf::from(&inside));
}

#[tokio::test]
async fn an_uploaded_picture_keeps_its_preview_on_the_device() {
    let rig = Rig::new();
    let attachment = stage_on_device(&rig, "shot.png", &picture()).await;
    let preview = attachment.thumbnail.clone().expect("a preview");
    assert!(preview.starts_with("data:image/"), "{preview}");
    let delivered = rig
        .transfers
        .deliver(&rig.local, vec![attachment.id])
        .await
        .unwrap();
    assert_eq!(delivered[0].attachment.thumbnail, Some(preview.clone()));
    assert_eq!(
        crate::sidecar::thumbnail_of(&delivered[0].path),
        Some(preview)
    );
    assert_eq!(delivered[0].text, None);
}

#[tokio::test]
async fn an_upload_to_a_container_brings_back_its_preview_and_text() {
    let rig = Rig::new();
    let src = source();
    fs::write(src.path().join("shot.png"), picture()).unwrap();
    fs::write(src.path().join("notes.docx"), b"words").unwrap();
    let workspace = UploadTarget::Workspace {
        workspace: rig.container.workspace,
    };
    let mut shot = Upload::new(workspace.clone(), &src.path().join("shot.png"));
    let shot = staged(&shot.run(&rig.transfers).await, shot.key);
    assert!(shot.thumbnail.is_some());
    let mut notes = Upload::new(workspace, &src.path().join("notes.docx"));
    let notes = staged(&notes.run(&rig.transfers).await, notes.key);
    assert_eq!(notes.thumbnail, None);

    let delivered = rig
        .transfers
        .deliver(&rig.boxed, vec![shot.id, notes.id])
        .await
        .unwrap();
    assert_eq!(delivered[0].attachment.thumbnail, shot.thumbnail);
    assert_eq!(delivered[0].text, None);
    let text = delivered[1].text.clone().expect("the document's text");
    assert_eq!(text, crate::doctext::text_path(&delivered[1].path));
    let local = rig.container.local(text.to_str().unwrap());
    assert_eq!(fs::read_to_string(local).unwrap(), "text of notes.docx");
}

#[tokio::test]
async fn delivering_a_document_into_a_container_has_its_guest_read_it() {
    let rig = Rig::new();
    let attachment = stage_on_device(&rig, "deck.pptx", b"slides").await;
    let delivered = rig
        .transfers
        .deliver(&rig.boxed, vec![attachment.id])
        .await
        .unwrap();
    let inside = delivered[0].path.to_str().unwrap();
    assert_eq!(
        delivered[0].text,
        Some(PathBuf::from(format!("{inside}.txt")))
    );
    let local = rig.container.local(&format!("{inside}.txt"));
    assert_eq!(fs::read_to_string(local).unwrap(), "text of deck.pptx");
}

#[test]
fn a_guest_cant_name_text_or_a_preview_that_isnt_beside_the_upload() {
    let transfer = TransferId::generate();
    let path = format!("{ATTACHMENTS_DIR}/staged/{transfer}/a.docx");
    let entries = [ManifestEntry {
        path: "a.docx".into(),
        kind: EntryKind::File,
        executable: false,
        size: 5,
        sha256: Some("0".repeat(64)),
    }];
    for text in [
        format!("{ATTACHMENTS_DIR}/staged/other/a.docx.txt"),
        "/etc/passwd".to_owned(),
        format!("{path}.md"),
    ] {
        let stored = GuestStored {
            paths: vec![path.clone()],
            thumbnail: Some("javascript:alert(1)".into()),
            text: Some(text.clone()),
        };
        let (attachment, kept) = guest_attachment(transfer, &entries, &stored).unwrap();
        assert_eq!((attachment.thumbnail, kept), (None, None), "{text}");
    }
    let stored = GuestStored {
        paths: vec![path.clone()],
        thumbnail: None,
        text: Some(format!("{path}.txt")),
    };
    let (_, kept) = guest_attachment(transfer, &entries, &stored).unwrap();
    assert_eq!(kept, Some(format!("{path}.txt")));
}

#[tokio::test]
async fn delivering_what_isnt_staged_says_its_missing() {
    let rig = Rig::new();
    let id = AttachmentId::generate();
    for place in [&rig.local, &rig.boxed] {
        assert!(matches!(
            rig.transfers.deliver(place, vec![id]).await,
            Err(DeliverError::Missing(missing)) if missing == id
        ));
    }
}

#[tokio::test]
async fn a_guest_answers_a_begin_sent_again_after_its_job_ends() {
    let rig = Rig::new();
    let src = source();
    let path = src.path().join("a.txt");
    fs::write(&path, b"again").unwrap();
    let walk = maxhq_transfer::walk(&path).unwrap();
    let (out, mut heard) = Heard::new();
    let transfer = TransferId::generate();
    let begin = HostToGuest::UploadBegin {
        transfer,
        agent: None,
        entries: walk.entries.clone(),
    };
    rig.container.files.message(begin.clone(), out.clone());
    assert!(matches!(
        heard.message().await,
        GuestToHost::TransferCredit { committed: 0, .. }
    ));
    rig.container.files.chunk(transfer, 0, b"again".to_vec());
    let GuestToHost::UploadStored { paths, .. } = heard.past_credit().await else {
        panic!("expected the upload stored");
    };
    let staged = rig
        .container
        .root
        .path()
        .join("attachments/staged")
        .join(transfer.to_string());
    assert_eq!(
        paths,
        vec![staged.join("a.txt").to_str().unwrap().to_owned()]
    );

    // Sent again with an agent named, it moves into the agent's folder.
    let agent = AgentId::generate();
    rig.container.files.message(
        HostToGuest::UploadBegin {
            transfer,
            agent: Some(agent),
            entries: walk.entries.clone(),
        },
        out.clone(),
    );
    let GuestToHost::UploadStored { paths, .. } = heard.past_credit().await else {
        panic!("expected the upload stored");
    };
    let moved = rig
        .container
        .root
        .path()
        .join("attachments")
        .join(agent.to_string())
        .join(transfer.to_string())
        .join("a.txt");
    assert_eq!(paths, vec![moved.to_str().unwrap().to_owned()]);
    assert_eq!(fs::read(&moved).unwrap(), b"again");
    assert!(!staged.exists());
}

#[tokio::test]
async fn a_guest_refuses_paths_that_escape_and_cleans_up_a_cancel() {
    let rig = Rig::new();
    let (out, mut heard) = Heard::new();
    let entry = |path: &str| ManifestEntry {
        path: path.to_owned(),
        kind: EntryKind::File,
        executable: false,
        size: 4,
        sha256: Some("0".repeat(64)),
    };
    for bad in ["../x", "/etc/x", "a/../../x"] {
        rig.container.files.message(
            HostToGuest::UploadBegin {
                transfer: TransferId::generate(),
                agent: None,
                entries: vec![entry(bad)],
            },
            out.clone(),
        );
        assert!(matches!(
            heard.message().await,
            GuestToHost::TransferFailed {
                code: ErrorCode::BadPath,
                ..
            }
        ));
    }
    for bad in ["/etc/hosts", "/home/agent/work/../../../etc/hosts"] {
        rig.container.files.message(
            HostToGuest::FetchBegin {
                transfer: TransferId::generate(),
                path: bad.to_owned(),
            },
            out.clone(),
        );
        assert!(matches!(
            heard.message().await,
            GuestToHost::TransferFailed {
                code: ErrorCode::BadPath,
                ..
            }
        ));
    }

    let transfer = TransferId::generate();
    rig.container.files.message(
        HostToGuest::UploadBegin {
            transfer,
            agent: None,
            entries: vec![entry("x.bin")],
        },
        out.clone(),
    );
    assert!(matches!(
        heard.message().await,
        GuestToHost::TransferCredit { .. }
    ));
    let incoming = rig
        .container
        .root
        .path()
        .join("attachments/incoming")
        .join(transfer.to_string());
    assert!(incoming.exists());
    rig.container
        .files
        .message(HostToGuest::UploadCancel { transfer }, out.clone());
    timeout(WAIT, async {
        while incoming.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the landing removed");
    heard.quiet().await;
}

/// Makes `dir` a git checkout that ignores `.env` and `build/`, with one
/// of each in it.
fn ignoring_checkout(dir: &Path) {
    // Through the guest's git, which leaves out what a git hook sets.
    let status = crate::guest::worktree::git(None)
        .args(["init", "-q"])
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success());
    fs::write(dir.join(".gitignore"), ".env\nbuild/\n").unwrap();
    fs::write(dir.join(".env"), b"KEY=1\n").unwrap();
    fs::create_dir_all(dir.join("build")).unwrap();
    fs::write(dir.join("build/out"), b"x").unwrap();
}

/// What an upload of a folder named `repo`, holding `.env`, stages for a
/// workspace.
async fn stage_a_copy(rig: &Rig, workspace: WorkspaceId, env: &[u8]) -> AttachmentRef {
    let src = source();
    let folder = src.path().join("repo");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join(".env"), env).unwrap();
    let mut upload = Upload::new(UploadTarget::Workspace { workspace }, &folder);
    staged(&upload.run(&rig.transfers).await, upload.key)
}

/// A clone made by `git init`, and a staged folder holding `path` as a hook
/// that touches a marker file. Returns the clone, the staged top and the marker.
fn staged_hook(temp: &TempDir, path: &str) -> (PathBuf, PathBuf, PathBuf) {
    let checkout = temp.path().join("clone");
    fs::create_dir_all(&checkout).unwrap();
    // Through the guest's git, which leaves out what a git hook sets.
    let init = crate::guest::worktree::git(None)
        .args(["init", "-q"])
        .current_dir(&checkout)
        .status()
        .unwrap();
    assert!(init.success());
    let marker = temp.path().join("hook-ran");
    let top = temp.path().join("staged/top");
    let hook = top.join(path);
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
    fs::set_permissions(&hook, Permissions::from_mode(0o755)).unwrap();
    (checkout, top, marker)
}

#[test]
fn a_staged_copy_cant_reach_the_git_folder_by_another_spelling() {
    for name in [
        ".GIT",
        ".Git",
        ".g\u{200c}it",
        ".git\u{feff}",
        "\u{202a}.GIT",
    ] {
        let temp = TempDir::new();
        let (checkout, top, marker) = staged_hook(&temp, &format!("{name}/hooks/pre-commit"));
        let placed = deliver::place(&top, &checkout, true, None).unwrap();
        assert_eq!(placed.written, 0, "{name:?}");
        assert_eq!(placed.failed.len(), 1, "{name:?}");
        assert_eq!(placed.failed[0].path, name);
        assert!(!checkout.join(".git/hooks/pre-commit").exists(), "{name:?}");

        let commit = crate::guest::worktree::git(None)
            .args(["-c", "user.name=a", "-c", "user.email=a@example.invalid"])
            .args(["commit", "-q", "--allow-empty", "-m", "x"])
            .current_dir(&checkout)
            .status()
            .unwrap();
        assert!(commit.success());
        assert!(!marker.exists(), "{name:?}");
    }
}

/// A name `skipped` knows nothing about still can't land on `.git`, which a
/// hard link stands in for here: no disk's folding is needed to make two names
/// one file. `.git` is a file in a linked worktree or a submodule.
#[test]
fn a_staged_copy_cant_replace_git_under_a_name_the_disk_resolves_to_it() {
    let temp = TempDir::new();
    let checkout = temp.path().join("clone");
    fs::create_dir_all(&checkout).unwrap();
    fs::write(checkout.join(".git"), b"gitdir: /somewhere\n").unwrap();
    fs::hard_link(checkout.join(".git"), checkout.join("other-name")).unwrap();
    let top = temp.path().join("staged/top");
    fs::create_dir_all(&top).unwrap();
    fs::write(top.join("other-name"), b"gitdir: /attacker\n").unwrap();

    let placed = deliver::place(&top, &checkout, true, None).unwrap();
    assert_eq!(placed.written, 0);
    assert_eq!(placed.failed.len(), 1);
    assert_eq!(
        fs::read(checkout.join(".git")).unwrap(),
        b"gitdir: /somewhere\n"
    );
}

fn env_file() -> IgnoredFile {
    IgnoredFile {
        path: ".env".into(),
        size: 6,
    }
}

#[tokio::test]
async fn a_folder_workspace_lists_and_takes_its_ignored_files() {
    let rig = Rig::new();
    ignoring_checkout(&rig.local.cwd);

    // A folder of ignored files comes as the folder, which a copy skips.
    let listed = rig.transfers.list_ignored(rig.local.workspace).await;
    assert_eq!(
        listed,
        ActionOutcome::IgnoredFiles {
            files: vec![env_file()],
            truncated: false,
        }
    );

    let (entries, bytes) = rig
        .fetch_files(rig.local.workspace, &[".env"])
        .await
        .unwrap();
    let paths: Vec<_> = entries.iter().map(|entry| entry.path.as_str()).collect();
    assert_eq!(paths, ["work", "work/.env"]);
    assert_eq!(bytes, b"KEY=1\n");
    assert_eq!(
        rig.fetch_files(rig.local.workspace, &["build"])
            .await
            .unwrap_err(),
        ErrorCode::BadPath
    );

    // What's there stays unless the copy says to overwrite it.
    let copy = stage_a_copy(&rig, rig.local.workspace, b"KEY=2\n").await;
    let placed = rig
        .transfers
        .place_staged(rig.local.workspace, copy.id, false)
        .await;
    assert_eq!(
        placed,
        ActionOutcome::Placed {
            written: 0,
            failed: vec![PlaceFailure {
                path: ".env".into(),
                reason: "a file is already there".into(),
            }],
        }
    );
    assert_eq!(fs::read(rig.local.cwd.join(".env")).unwrap(), b"KEY=1\n");
    assert!(!Path::new(&copy.path).exists());

    let copy = stage_a_copy(&rig, rig.local.workspace, b"KEY=2\n").await;
    let placed = rig
        .transfers
        .place_staged(rig.local.workspace, copy.id, true)
        .await;
    assert_eq!(
        placed,
        ActionOutcome::Placed {
            written: 1,
            failed: Vec::new(),
        }
    );
    assert_eq!(fs::read(rig.local.cwd.join(".env")).unwrap(), b"KEY=2\n");

    // Placed once, the copy isn't staged any more.
    assert!(matches!(
        rig.transfers
            .place_staged(rig.local.workspace, copy.id, true)
            .await,
        ActionOutcome::Refused { .. }
    ));
}

#[tokio::test]
async fn a_container_workspace_lists_and_takes_its_ignored_files_through_its_guest() {
    let rig = Rig::new();
    let repo = rig.container.root.path().join("work/repo");
    ignoring_checkout(&repo);

    let listed = rig.transfers.list_ignored(rig.container.workspace).await;
    assert_eq!(
        listed,
        ActionOutcome::IgnoredFiles {
            files: vec![env_file()],
            truncated: false,
        }
    );

    let (entries, bytes) = rig
        .fetch_files(rig.container.workspace, &[".env"])
        .await
        .unwrap();
    let paths: Vec<_> = entries.iter().map(|entry| entry.path.as_str()).collect();
    assert_eq!(paths, ["repo", "repo/.env"]);
    assert_eq!(bytes, b"KEY=1\n");
    assert_eq!(
        rig.fetch_files(rig.container.workspace, &["../../etc/hosts"])
            .await
            .unwrap_err(),
        ErrorCode::BadPath
    );

    fs::remove_file(repo.join(".env")).unwrap();
    let copy = stage_a_copy(&rig, rig.container.workspace, b"KEY=2\n").await;
    let placed = rig
        .transfers
        .place_staged(rig.container.workspace, copy.id, false)
        .await;
    assert_eq!(
        placed,
        ActionOutcome::Placed {
            written: 1,
            failed: Vec::new(),
        }
    );
    assert_eq!(fs::read(repo.join(".env")).unwrap(), b"KEY=2\n");
    assert!(!rig.container.local(&copy.path).exists());

    // A workspace this device doesn't know is refused.
    assert!(matches!(
        rig.transfers.list_ignored(WorkspaceId::generate()).await,
        ActionOutcome::Refused { .. }
    ));
}

#[tokio::test]
async fn a_container_cant_add_a_file_to_a_copy_of_chosen_files() {
    let rig = Rig::new();
    let repo = rig.container.root.path().join("work/repo");
    ignoring_checkout(&repo);
    // Empty, so without the check the download ends instead of waiting for it.
    rig.container.adds.lock().unwrap().push(ManifestEntry {
        path: "repo/.envrc".to_owned(),
        kind: EntryKind::File,
        executable: false,
        size: 0,
        sha256: Some("0".repeat(64)),
    });

    assert_eq!(
        rig.fetch_files(rig.container.workspace, &[".env"])
            .await
            .unwrap_err(),
        ErrorCode::BadPath
    );
}

/// Hears what a guest sends, without a host behind it.
struct Heard {
    tx: mpsc::UnboundedSender<GuestToHost>,
}

/// What a [`Heard`] heard.
struct Hearing(mpsc::UnboundedReceiver<GuestToHost>);

impl Heard {
    fn new() -> (Arc<Self>, Hearing) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Arc::new(Self { tx }), Hearing(rx))
    }
}

impl Out for Heard {
    fn message(&self, message: GuestToHost) -> BoxFuture<Result<(), Closed>> {
        let sent = self.tx.send(message).map_err(|_| Closed);
        Box::pin(async move { sent })
    }

    fn chunk(&self, _: TransferId, _: u64, _: Vec<u8>) -> BoxFuture<Result<(), Closed>> {
        Box::pin(async { Ok(()) })
    }
}

impl Hearing {
    async fn message(&mut self) -> GuestToHost {
        timeout(WAIT, self.0.recv())
            .await
            .expect("a message in time")
            .expect("the guest keeps its end")
    }

    /// The next message that isn't credit.
    async fn past_credit(&mut self) -> GuestToHost {
        loop {
            match self.message().await {
                GuestToHost::TransferCredit { .. } => {}
                other => return other,
            }
        }
    }

    async fn quiet(&mut self) {
        let next = timeout(Duration::from_millis(300), self.0.recv()).await;
        assert!(next.is_err(), "expected nothing, got {next:?}");
    }
}

#[tokio::test]
async fn a_deep_or_unreadable_leftover_doesnt_stop_a_quota_and_goes_when_removed() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new();
    let root = dir.path().join("uploads");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("kept"), [0; 10]).unwrap();
    let shut = root.join("shut");
    fs::create_dir(&shut).unwrap();
    fs::set_permissions(&shut, fs::Permissions::from_mode(0o000)).unwrap();
    let deep = root.join("deep");
    fs::create_dir(&deep).unwrap();
    let mut at = maxhq_transfer::beneath::Dir::open(&deep).unwrap();
    for _ in 0..600 {
        at.make_dir("d", 0o755).unwrap();
        at = at.dir(&["d".to_owned()]).unwrap();
    }

    let checked = disk::check_quota(root.clone(), root.join("incoming"), 1, 11).await;
    fs::set_permissions(&shut, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(checked.is_ok(), "{checked:?}");

    disk::remove(&deep).await;
    assert!(!deep.exists());
}

#[tokio::test]
async fn a_send_of_a_picture_too_large_to_decode_goes_without_a_preview() {
    let name = "transfer::tests::a_send_of_a_picture_too_large_to_decode_goes_without_a_preview";
    if crate::testing::apart(name) {
        return;
    }
    let rig = Rig::new();
    fs::write(rig.local.cwd.join("huge.png"), crate::testing::HUGE_PNG).unwrap();
    let sent = rig
        .transfers
        .collect_files(&rig.local, "call", &["huge.png".to_owned()])
        .await
        .unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].thumbnail, None);
}

#[tokio::test]
async fn a_containers_send_of_a_picture_too_large_to_decode_goes_without_a_preview() {
    let name =
        "transfer::tests::a_containers_send_of_a_picture_too_large_to_decode_goes_without_a_preview";
    if crate::testing::apart(name) {
        return;
    }
    let rig = Rig::new();
    let repo = rig.container.root.path().join("work/repo");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join("huge.png"), crate::testing::HUGE_PNG).unwrap();
    let sent = rig
        .transfers
        .collect_files(&rig.boxed, "call", &["huge.png".to_owned()])
        .await
        .unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].thumbnail, None);
}

#[tokio::test]
async fn an_upload_of_a_picture_too_large_to_decode_stages_without_a_preview() {
    let name =
        "transfer::tests::an_upload_of_a_picture_too_large_to_decode_stages_without_a_preview";
    if crate::testing::apart(name) {
        return;
    }
    let rig = Rig::new();
    let staged = stage_on_device(&rig, "huge.png", &crate::testing::HUGE_PNG).await;
    assert_eq!(staged.thumbnail, None);
}
