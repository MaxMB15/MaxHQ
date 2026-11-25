// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Uploads and downloads over a link, against the fake `maxd`.

use std::collections::VecDeque;
use std::path::Path;

use maxhq_protocol::ids::{IdempotencyKey, TransferId, WorkspaceId};
use maxhq_protocol::message::{ErrorCode, ManifestEntry, UploadSource, SHOW_ANYWAY_SINCE};
use maxhq_protocol::timeline::{AttachmentKind, AttachmentRef};
use maxhq_protocol::OLDEST_PROTOCOL_VERSION;
use maxhq_transfer::send::RESTARTS;
use maxhq_transfer::{walk, Outgoing, Receiver, Room, Source, Walk};

use super::*;
use crate::media::Media;

/// No room kept free, so the fake device takes what comes on any disk.
const ANY_ROOM: Room = Room {
    keep_bytes: 0,
    keep_percent: 0,
    keep_at_most: 0,
};

/// A folder of its own for one test, removed when it drops.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("maxhq-client-{}", IdempotencyKey::generate()));
        std::fs::create_dir_all(&path).unwrap();
        Self(std::fs::canonicalize(path).unwrap())
    }

    fn join(&self, path: &str) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A frame from the client.
#[derive(Debug)]
enum Got {
    Message(ClientMessage),
    Chunk {
        transfer: TransferId,
        offset: u64,
        data: Vec<u8>,
    },
}

/// The fake `maxd` of a device that does transfers.
struct Device {
    maxd: FakeMaxd,
    /// What came before the sync in a hello, handed out first.
    held: VecDeque<Got>,
}

impl Device {
    /// Answers the next dial.
    async fn dialed(rig: &mut Rig) -> Self {
        Self::speaking(rig, ProtocolRange::OURS).await
    }

    /// Answers the next dial as a `maxd` that speaks `protocol`.
    async fn speaking(rig: &mut Rig, protocol: ProtocolRange) -> Self {
        let mut device = Self {
            maxd: rig.next_maxd().await,
            held: VecDeque::new(),
        };
        match device.got().await {
            Got::Message(ClientMessage::Hello(_)) => {}
            other => panic!("expected a hello, got {other:?}"),
        }
        device
            .send(&ServerMessage::Hello(ServerHello {
                protocol,
                maxd_version: "0.1.0".into(),
                device: super::device(),
                boot_id: BootId("boot-1".into()),
                instance: InstanceId::generate(),
                capabilities: Capabilities {
                    transfers: true,
                    ..Capabilities::default()
                },
                build: None,
                target: None,
                challenge: None,
            }))
            .await;
        // A transfer may send its begin again before the sync goes out.
        let mut held = VecDeque::new();
        loop {
            match device.got().await {
                Got::Message(ClientMessage::Sync { .. }) => break,
                other => held.push_back(other),
            }
        }
        device.held = held;
        device
    }

    async fn got(&mut self) -> Got {
        if let Some(got) = self.held.pop_front() {
            return got;
        }
        let mut buf = vec![0; 64 * 1024];
        loop {
            match self.maxd.decoder.next_frame().unwrap() {
                Some(Frame::Message(json)) => {
                    return Got::Message(serde_json::from_slice(json).unwrap());
                }
                Some(Frame::TransferChunk {
                    transfer,
                    offset,
                    data,
                }) => {
                    return Got::Chunk {
                        transfer,
                        offset,
                        data: data.to_vec(),
                    };
                }
                Some(other) => panic!("expected a message or file data, got {other:?}"),
                None => {}
            }
            let read = tokio::time::timeout(PATIENCE, self.maxd.stream.read(&mut buf))
                .await
                .expect("a frame in time")
                .unwrap();
            assert_ne!(read, 0, "the client closed the connection");
            self.maxd.decoder.push(&buf[..read]);
        }
    }

    /// The next message, past any file data.
    async fn message(&mut self) -> ClientMessage {
        loop {
            if let Got::Message(message) = self.got().await {
                return message;
            }
        }
    }

    /// The next chunk of `transfer`'s file data.
    async fn chunk(&mut self, transfer: TransferId) -> (u64, Vec<u8>) {
        match self.got().await {
            Got::Chunk {
                transfer: of,
                offset,
                data,
            } => {
                assert_eq!(of, transfer);
                (offset, data)
            }
            Got::Message(other) => panic!("expected file data, got {other:?}"),
        }
    }

    /// Takes file data until `bytes` of it came.
    async fn skip(&mut self, transfer: TransferId, bytes: u64) {
        let mut came = 0;
        while came < bytes {
            came += self.chunk(transfer).await.1.len() as u64;
        }
    }

    async fn send(&mut self, message: &ServerMessage) {
        self.maxd.send(message).await;
    }

    async fn credit(&mut self, transfer: TransferId, (committed, window_end): (u64, u64)) {
        self.send(&ServerMessage::TransferCredit {
            transfer,
            committed,
            window_end,
        })
        .await;
    }

    async fn failed(&mut self, transfer: TransferId, code: ErrorCode, message: &str) {
        self.send(&ServerMessage::TransferFailed {
            transfer,
            code,
            entry: None,
            message: message.into(),
        })
        .await;
    }

    async fn staged(&mut self, begin: &Begin, attachment: &AttachmentRef) {
        self.send(&ServerMessage::UploadDone {
            transfer: begin.transfer,
            attachment: attachment.clone(),
        })
        .await;
        self.send(&ServerMessage::ActionResult {
            key: begin.key,
            outcome: ActionOutcome::Staged {
                attachment: attachment.clone(),
            },
        })
        .await;
    }

    /// Takes file data into `receiver` and credits it until every byte is
    /// in.
    async fn land(&mut self, receiver: &mut Receiver, transfer: TransferId) {
        while !receiver.is_done() {
            let (offset, data) = self.chunk(transfer).await;
            receiver.write(offset, &data, now()).unwrap();
            if let Some(credit) = receiver.credit(now()) {
                self.credit(transfer, credit).await;
            }
        }
    }

    /// Sends what `walked` holds as the client's credit allows.
    async fn serve(&mut self, transfer: TransferId, walked: &Walk) {
        let mut source = Source::new(walked);
        let mut outgoing = Outgoing::new(walked.total());
        let mut resumed = false;
        loop {
            while let Some((offset, len)) = outgoing.next(frame::MAX_TRANSFER_CHUNK) {
                let data = source.read(offset, len).unwrap();
                let mut out = Vec::new();
                frame::encode_transfer_chunks(&mut out, transfer, offset, &data);
                self.maxd.stream.write_all(&out).await.unwrap();
                outgoing.sent_chunk(data.len());
            }
            if outgoing.is_sent() {
                return;
            }
            match self.message().await {
                ClientMessage::TransferCredit {
                    transfer: of,
                    committed,
                    window_end,
                } => {
                    assert_eq!(of, transfer);
                    if resumed {
                        outgoing.credit(committed, window_end).unwrap();
                    } else {
                        outgoing.resume(committed, window_end).unwrap();
                        resumed = true;
                    }
                }
                other => panic!("expected credit, got {other:?}"),
            }
        }
    }
}

/// What an upload's begin said.
struct Begin {
    key: IdempotencyKey,
    transfer: TransferId,
    target: UploadTarget,
    name: String,
    entries: Vec<ManifestEntry>,
}

fn upload_begin(message: ClientMessage) -> Begin {
    match message {
        ClientMessage::UploadBegin {
            key,
            transfer,
            target,
            source: UploadSource::Stream { name, entries },
        } => Begin {
            key,
            transfer,
            target,
            name,
            entries,
        },
        other => panic!("expected an upload, got {other:?}"),
    }
}

/// Opens a local link to a fake device that does transfers, with the
/// client's folders in `dir`.
async fn linked(dir: &TempDir) -> (Rig, LinkHandle, mpsc::Receiver<LinkEvent>, Device) {
    let mut rig = rig(dir);
    let (handle, mut events) = rig.client.link(spec(Route::Local));
    let device = Device::dialed(&mut rig).await;
    while !is_connected(&next_state(&mut events).await) {}
    (rig, handle, events, device)
}

/// A rig whose client keeps its downloads and staged copies in `dir`.
fn rig(dir: &TempDir) -> Rig {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    Arc::get_mut(&mut rig.client.shared)
        .expect("no link yet")
        .places = Places {
        downloads: dir.join("Downloads/MaxHQ"),
        staging: dir.join("staging"),
        media: dir.join("media"),
        room: ANY_ROOM,
    };
    rig
}

/// Drops the connection and answers the link's next dial.
async fn reconnect(
    rig: &mut Rig,
    events: &mut mpsc::Receiver<LinkEvent>,
    device: Device,
) -> Device {
    drop(device);
    assert!(!is_connected(&next_state(events).await));
    let device = Device::dialed(rig).await;
    while !is_connected(&next_state(events).await) {}
    device
}

/// Every progress report, in order.
#[derive(Default)]
struct Reports(Mutex<Vec<TransferProgress>>);

impl Reports {
    fn hear(&self) -> impl Fn(TransferProgress) + Send + Sync + '_ {
        |progress| self.0.lock().unwrap().push(progress)
    }

    fn states(&self) -> Vec<TransferState> {
        self.0.lock().unwrap().iter().map(|p| p.state).collect()
    }

    fn last(&self) -> TransferProgress {
        self.0.lock().unwrap().last().cloned().expect("a report")
    }
}

fn attachment(name: &str, kind: AttachmentKind, bytes: u64) -> AttachmentRef {
    AttachmentRef {
        id: "0199a7c2-0000-7000-8000-000000000003".parse().unwrap(),
        name: name.into(),
        kind,
        bytes,
        mime: None,
        path: format!("/attachments/{name}"),
        thumbnail: None,
    }
}

fn agent() -> AgentId {
    AgentId::derive("agent")
}

/// Bytes that aren't all the same, so a misplaced chunk shows.
fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

fn now() -> std::time::Instant {
    std::time::Instant::now()
}

#[tokio::test]
async fn a_folder_uploads_to_the_device_from_a_copy() {
    let dir = TempDir::new();
    let pics = dir.join("pics");
    std::fs::create_dir_all(pics.join("sub")).unwrap();
    std::fs::write(pics.join("a.txt"), "alpha").unwrap();
    let big = pattern(600 * 1024);
    std::fs::write(pics.join("sub/b.bin"), &big).unwrap();
    std::os::unix::fs::symlink("a.txt", pics.join("link")).unwrap();
    std::fs::write(pics.join(".DS_Store"), "junk").unwrap();
    let total = big.len() as u64 + 5;
    let (_rig, handle, _events, mut device) = linked(&dir).await;
    let reports = Reports::default();
    let staged = attachment("pics", AttachmentKind::Folder, total);

    let (results, ()) = tokio::join!(
        handle.upload(UploadTarget::Device, vec![pics.clone()], reports.hear()),
        async {
            let begin = upload_begin(device.message().await);
            assert_eq!(begin.target, UploadTarget::Device);
            assert_eq!(begin.name, "pics");
            let mut paths: Vec<&str> = begin.entries.iter().map(|e| e.path.as_str()).collect();
            paths.sort_unstable();
            assert_eq!(
                paths,
                [
                    "pics",
                    "pics/a.txt",
                    "pics/link",
                    "pics/sub",
                    "pics/sub/b.bin"
                ]
            );
            let mut receiver =
                Receiver::open(&dir.join("device"), &begin.entries, ANY_ROOM).unwrap();
            device
                .credit(begin.transfer, receiver.resume(now()).unwrap())
                .await;
            device.land(&mut receiver, begin.transfer).await;
            device.staged(&begin, &staged).await;
        }
    );

    assert_eq!(results, [Ok(ActionOutcome::Staged { attachment: staged })]);
    let landed = dir.join("device/pics");
    assert_eq!(std::fs::read(landed.join("sub/b.bin")).unwrap(), big);
    assert_eq!(std::fs::read(landed.join("a.txt")).unwrap(), b"alpha");
    assert_eq!(
        std::fs::read_link(landed.join("link")).unwrap(),
        Path::new("a.txt")
    );
    // The copy goes once the upload ends.
    assert_eq!(std::fs::read_dir(dir.join("staging")).unwrap().count(), 0);
    let last = reports.last();
    assert_eq!(
        (last.state, last.sent, last.total),
        (TransferState::Done, total, total)
    );
}

#[tokio::test]
async fn an_upload_picks_up_where_the_device_got_to_after_a_drop() {
    let dir = TempDir::new();
    let file = dir.join("big.bin");
    let bytes = pattern(3 * 1024 * 1024);
    std::fs::write(&file, &bytes).unwrap();
    let (mut rig, handle, mut events, mut device) = linked(&dir).await;
    let reports = Reports::default();
    let staged = attachment("big.bin", AttachmentKind::File, bytes.len() as u64);
    let root = dir.join("device");

    let (results, ()) = tokio::join!(
        handle.upload(UploadTarget::Device, vec![file.clone()], reports.hear()),
        async {
            let first = upload_begin(device.message().await);
            let mut receiver = Receiver::open(&root, &first.entries, ANY_ROOM).unwrap();
            device
                .credit(first.transfer, receiver.resume(now()).unwrap())
                .await;
            while receiver.written() < 1024 * 1024 {
                let (offset, data) = device.chunk(first.transfer).await;
                receiver.write(offset, &data, now()).unwrap();
                if let Some(credit) = receiver.credit(now()) {
                    device.credit(first.transfer, credit).await;
                }
            }
            receiver.sync(now()).unwrap();
            let committed = receiver.committed();
            assert!(committed > 0);
            drop(receiver);

            let mut device = reconnect(&mut rig, &mut events, device).await;
            let again = upload_begin(device.message().await);
            assert_eq!((again.key, again.transfer), (first.key, first.transfer));
            let mut receiver = Receiver::open(&root, &again.entries, ANY_ROOM).unwrap();
            let credit = receiver.resume(now()).unwrap();
            assert_eq!(credit.0, committed);
            device.credit(again.transfer, credit).await;
            let (offset, data) = device.chunk(again.transfer).await;
            assert_eq!(offset, committed);
            receiver.write(offset, &data, now()).unwrap();
            device.land(&mut receiver, again.transfer).await;
            device.staged(&again, &staged).await;
        }
    );

    assert_eq!(results, [Ok(ActionOutcome::Staged { attachment: staged })]);
    assert_eq!(std::fs::read(root.join("big.bin")).unwrap(), bytes);
    assert!(reports.states().contains(&TransferState::Paused));
    assert_eq!(reports.last().state, TransferState::Done);
}

#[tokio::test]
async fn an_upload_whose_hash_didnt_match_goes_again_under_the_same_transfer() {
    let dir = TempDir::new();
    let bytes = pattern(100 * 1024);
    let total = bytes.len() as u64;
    let (_rig, handle, _events, mut device) = linked(&dir).await;
    let staged = attachment("notes.txt", AttachmentKind::File, total);
    let root = dir.join("device");

    let (result, ()) = tokio::join!(
        handle.upload_bytes(
            UploadTarget::Device,
            "notes.txt".into(),
            "text/plain",
            bytes.clone(),
            |_| {},
        ),
        async {
            let first = upload_begin(device.message().await);
            assert_eq!(first.name, "notes.txt");
            device.credit(first.transfer, (0, total)).await;
            device.skip(first.transfer, total).await;
            device
                .failed(first.transfer, ErrorCode::HashMismatch, "notes.txt changed")
                .await;

            let again = upload_begin(device.message().await);
            assert_eq!((again.key, again.transfer), (first.key, first.transfer));
            let mut receiver = Receiver::open(&root, &again.entries, ANY_ROOM).unwrap();
            device
                .credit(again.transfer, receiver.resume(now()).unwrap())
                .await;
            device.land(&mut receiver, again.transfer).await;
            device.staged(&again, &staged).await;
        }
    );

    assert_eq!(result, Ok(ActionOutcome::Staged { attachment: staged }));
    assert_eq!(std::fs::read(root.join("notes.txt")).unwrap(), bytes);
}

#[tokio::test]
async fn an_upload_whose_hash_never_matches_gives_up_and_cancels() {
    let dir = TempDir::new();
    let bytes = pattern(1024);
    let total = bytes.len() as u64;
    let (_rig, handle, _events, mut device) = linked(&dir).await;

    let (result, ()) = tokio::join!(
        handle.upload_bytes(
            UploadTarget::Device,
            "notes.txt".into(),
            "text/plain",
            bytes,
            |_| {},
        ),
        async {
            let first = upload_begin(device.message().await);
            for attempt in 0..=RESTARTS {
                if attempt > 0 {
                    let again = upload_begin(device.message().await);
                    assert_eq!((again.key, again.transfer), (first.key, first.transfer));
                }
                device.credit(first.transfer, (0, total)).await;
                device.skip(first.transfer, total).await;
                device
                    .failed(first.transfer, ErrorCode::HashMismatch, "notes.txt changed")
                    .await;
            }
            assert_eq!(
                device.message().await,
                ClientMessage::UploadCancel {
                    transfer: first.transfer
                }
            );
        }
    );

    assert!(matches!(
        result,
        Err(TransferError::Failed {
            code: ErrorCode::HashMismatch,
            ..
        })
    ));
}

#[tokio::test]
async fn a_cancelled_upload_ends_and_tells_the_device() {
    let dir = TempDir::new();
    let bytes = pattern(1024 * 1024);
    let (_rig, handle, _events, mut device) = linked(&dir).await;

    let (result, ()) = tokio::join!(
        handle.upload_bytes(UploadTarget::Device, "big".into(), "", bytes, |_| {}),
        async {
            let begin = upload_begin(device.message().await);
            let chunk = frame::MAX_TRANSFER_CHUNK as u64;
            device.credit(begin.transfer, (0, chunk)).await;
            device.skip(begin.transfer, chunk).await;
            handle.cancel_transfer(begin.transfer);
            assert_eq!(
                device.message().await,
                ClientMessage::UploadCancel {
                    transfer: begin.transfer
                }
            );
        }
    );

    assert_eq!(result, Err(TransferError::Cancelled));
}

#[tokio::test]
async fn an_upload_the_device_refuses_fails_with_its_reason() {
    let dir = TempDir::new();
    let (_rig, handle, _events, mut device) = linked(&dir).await;
    let reason = "that agent is gone";
    let target = UploadTarget::Agent { agent: agent() };

    let (result, ()) = tokio::join!(
        handle.upload_bytes(target.clone(), "a".into(), "", vec![1], |_| {}),
        async {
            let begin = upload_begin(device.message().await);
            assert_eq!(begin.target, target);
            device
                .failed(begin.transfer, ErrorCode::NoSuchAgent, reason)
                .await;
            device
                .send(&ServerMessage::ActionResult {
                    key: begin.key,
                    outcome: ActionOutcome::UnknownAgent,
                })
                .await;
        }
    );

    assert_eq!(
        result,
        Err(TransferError::Failed {
            code: ErrorCode::NoSuchAgent,
            message: reason.into(),
        })
    );
    // The device ended it, so no cancel goes before the next request.
    handle.send(mark_seen()).await.unwrap();
    assert_eq!(device.message().await, mark_seen());
}

#[tokio::test]
async fn a_path_with_nothing_there_is_refused_before_it_goes() {
    let dir = TempDir::new();
    let (_rig, handle, _events, mut device) = linked(&dir).await;

    let results = handle
        .upload(UploadTarget::Device, vec![dir.join("missing")], |_| {})
        .await;

    assert!(matches!(
        results.as_slice(),
        [Err(TransferError::Failed {
            code: ErrorCode::BadPath,
            ..
        })]
    ));
    // Neither a begin nor a cancel went out.
    handle.send(mark_seen()).await.unwrap();
    assert_eq!(device.message().await, mark_seen());
}

#[tokio::test]
async fn a_download_lands_under_a_free_name_marked_as_from_elsewhere() {
    let dir = TempDir::new();
    let downloads = dir.join("Downloads/MaxHQ");
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::write(downloads.join("report.txt"), "old").unwrap();
    // Taken too, on a disk that doesn't tell case apart.
    std::fs::write(downloads.join("REPORT 2.txt"), "older").unwrap();
    let shared = dir.join("agent");
    std::fs::create_dir_all(&shared).unwrap();
    let bytes = pattern(1500 * 1024);
    std::fs::write(shared.join("report.txt"), &bytes).unwrap();
    let walked = walk(&shared.join("report.txt")).unwrap();
    let (_rig, handle, _events, mut device) = linked(&dir).await;
    let reports = Reports::default();

    let (saved, transfer) = tokio::join!(
        handle.download(agent(), "/work/report.txt".into(), reports.hear()),
        async {
            let (key, transfer) = match device.message().await {
                ClientMessage::FetchBegin {
                    key,
                    transfer,
                    agent: of,
                    path,
                    ..
                } => {
                    assert_eq!((of, path.as_str()), (agent(), "/work/report.txt"));
                    (key, transfer)
                }
                other => panic!("expected a fetch, got {other:?}"),
            };
            device
                .send(&ServerMessage::FetchManifest {
                    transfer,
                    entries: walked.entries.clone(),
                })
                .await;
            device.serve(transfer, &walked).await;
            device
                .send(&ServerMessage::ActionResult {
                    key,
                    outcome: ActionOutcome::Done,
                })
                .await;
            transfer
        }
    );

    let saved = saved.unwrap();
    assert_eq!(saved, downloads.join("report 3.txt"));
    assert_eq!(std::fs::read(&saved).unwrap(), bytes);
    #[cfg(target_os = "macos")]
    {
        let mut mark = [0; 128];
        let len = rustix::fs::lgetxattr(&saved, "com.apple.quarantine", &mut mark).unwrap();
        assert!(mark[..len].starts_with(b"0081;"));
    }
    let incoming = downloads.join(".incoming");
    assert!(!incoming.join(transfer.to_string()).exists());
    let last = reports.last();
    assert_eq!(
        (last.transfer, last.state, last.sent),
        (transfer, TransferState::Done, bytes.len() as u64)
    );
}

#[tokio::test]
async fn a_download_lands_in_the_folder_the_client_was_given() {
    let dir = TempDir::new();
    let shared = dir.join("agent");
    std::fs::create_dir_all(&shared).unwrap();
    let bytes = pattern(64 * 1024);
    std::fs::write(shared.join("notes.txt"), &bytes).unwrap();
    let walked = walk(&shared.join("notes.txt")).unwrap();
    let folder = dir.join("Elsewhere/MaxHQ");
    let mut rig = rig(&dir);
    rig.client = rig.client.downloading_into(folder.clone());
    let (handle, mut events) = rig.client.link(spec(Route::Local));
    let mut device = Device::dialed(&mut rig).await;
    while !is_connected(&next_state(&mut events).await) {}

    let (saved, ()) = tokio::join!(
        handle.download(agent(), "/work/notes.txt".into(), |_| {}),
        async {
            let ClientMessage::FetchBegin { key, transfer, .. } = device.message().await else {
                panic!("expected a fetch");
            };
            device
                .send(&ServerMessage::FetchManifest {
                    transfer,
                    entries: walked.entries.clone(),
                })
                .await;
            device.serve(transfer, &walked).await;
            device
                .send(&ServerMessage::ActionResult {
                    key,
                    outcome: ActionOutcome::Done,
                })
                .await;
        }
    );

    let saved = saved.unwrap();
    assert_eq!(saved, folder.join("notes.txt"));
    assert_eq!(std::fs::read(&saved).unwrap(), bytes);
    assert!(!dir.join("Downloads/MaxHQ/notes.txt").exists());
}

#[tokio::test]
async fn a_download_the_device_refuses_fails_with_its_reason() {
    let dir = TempDir::new();
    let (_rig, handle, _events, mut device) = linked(&dir).await;
    let reason = "that's outside the agent's folder";

    let (result, transfer) = tokio::join!(
        handle.download(agent(), "/etc/passwd".into(), |_| {}),
        async {
            let ClientMessage::FetchBegin { key, transfer, .. } = device.message().await else {
                panic!("expected a fetch");
            };
            device.failed(transfer, ErrorCode::NotAllowed, reason).await;
            device
                .send(&ServerMessage::ActionResult {
                    key,
                    outcome: ActionOutcome::Refused {
                        reason: reason.into(),
                    },
                })
                .await;
            transfer
        }
    );

    assert_eq!(
        result,
        Err(TransferError::Failed {
            code: ErrorCode::NotAllowed,
            message: reason.into(),
        })
    );
    let incoming = dir.join("Downloads/MaxHQ/.incoming");
    assert!(!incoming.join(transfer.to_string()).exists());
    handle.send(mark_seen()).await.unwrap();
    assert_eq!(device.message().await, mark_seen());
}

#[tokio::test]
async fn a_copy_lands_in_staging_and_goes_when_dropped() {
    let dir = TempDir::new();
    let shared = dir.join("agent");
    std::fs::create_dir_all(&shared).unwrap();
    let bytes = pattern(200 * 1024);
    std::fs::write(shared.join("shot.png"), &bytes).unwrap();
    let walked = walk(&shared.join("shot.png")).unwrap();
    let (_rig, handle, _events, mut device) = linked(&dir).await;
    let reports = Reports::default();

    let (copied, transfer) = tokio::join!(
        handle.copy(agent(), "/work/shot.png".into(), reports.hear()),
        async {
            let ClientMessage::FetchBegin { key, transfer, .. } = device.message().await else {
                panic!("expected a fetch");
            };
            device
                .send(&ServerMessage::FetchManifest {
                    transfer,
                    entries: walked.entries.clone(),
                })
                .await;
            device.serve(transfer, &walked).await;
            device
                .send(&ServerMessage::ActionResult {
                    key,
                    outcome: ActionOutcome::Done,
                })
                .await;
            transfer
        }
    );

    let copied = copied.unwrap();
    let folder = dir.join("staging").join(transfer.to_string());
    assert_eq!(copied.path(), folder.join("shot.png"));
    assert_eq!(std::fs::read(copied.path()).unwrap(), bytes);
    assert!(!dir.join("Downloads/MaxHQ").join("shot.png").exists());
    assert_eq!(reports.last().state, TransferState::Done);

    drop(copied);
    for _ in 0..200 {
        if !folder.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(!folder.exists());
}

/// The phone hands the client its own staging folder, since Android has no
/// usable cache or temporary folder outside the app's.
#[tokio::test]
async fn a_copy_lands_in_the_staging_folder_the_client_was_given() {
    let dir = TempDir::new();
    let shared = dir.join("agent");
    std::fs::create_dir_all(&shared).unwrap();
    let bytes = pattern(64 * 1024);
    std::fs::write(shared.join("shot.png"), &bytes).unwrap();
    let walked = walk(&shared.join("shot.png")).unwrap();
    let folder = dir.join("app/cache/staging");
    let mut rig = rig(&dir);
    rig.client = rig.client.staging_in(folder.clone());
    let (handle, mut events) = rig.client.link(spec(Route::Local));
    let mut device = Device::dialed(&mut rig).await;
    while !is_connected(&next_state(&mut events).await) {}

    let (copied, transfer) = tokio::join!(
        handle.copy(agent(), "/work/shot.png".into(), |_| {}),
        async {
            let ClientMessage::FetchBegin { key, transfer, .. } = device.message().await else {
                panic!("expected a fetch");
            };
            device
                .send(&ServerMessage::FetchManifest {
                    transfer,
                    entries: walked.entries.clone(),
                })
                .await;
            device.serve(transfer, &walked).await;
            device
                .send(&ServerMessage::ActionResult {
                    key,
                    outcome: ActionOutcome::Done,
                })
                .await;
            transfer
        }
    );

    let copied = copied.unwrap();
    let staged = folder.join(transfer.to_string()).join("shot.png");
    assert_eq!(copied.path(), staged);
    assert_eq!(std::fs::read(copied.path()).unwrap(), bytes);
    assert!(!dir.join("staging").exists());
}

#[tokio::test]
async fn files_from_a_workspace_land_in_staging_as_one_folder() {
    let dir = TempDir::new();
    let files = dir.join("checkout/files");
    std::fs::create_dir_all(files.join("app")).unwrap();
    std::fs::write(files.join(".env"), "TOKEN=1").unwrap();
    std::fs::write(files.join("app/.env.local"), [0, 159, 146, 150]).unwrap();
    let walked = walk(&files).unwrap();
    let (_rig, handle, _events, mut device) = linked(&dir).await;
    let workspace = WorkspaceId::derive("w");
    let paths = vec![".env".to_owned(), "app/.env.local".to_owned()];

    let (copied, transfer) =
        tokio::join!(handle.copy_files(workspace, paths.clone(), |_| {}), async {
            let ClientMessage::FetchFiles {
                key,
                transfer,
                workspace: from,
                paths: asked,
            } = device.message().await
            else {
                panic!("expected a fetch of files");
            };
            assert_eq!((from, asked), (workspace, paths));
            device
                .send(&ServerMessage::FetchManifest {
                    transfer,
                    entries: walked.entries.clone(),
                })
                .await;
            device.serve(transfer, &walked).await;
            device
                .send(&ServerMessage::ActionResult {
                    key,
                    outcome: ActionOutcome::Done,
                })
                .await;
            transfer
        });

    let copied = copied.unwrap();
    let folder = dir.join("staging").join(transfer.to_string());
    assert_eq!(copied.path(), folder.join("files"));
    assert_eq!(
        std::fs::read(copied.path().join("app/.env.local")).unwrap(),
        [0, 159, 146, 150]
    );
    drop(copied);
    for _ in 0..200 {
        if !folder.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(!folder.exists());
}

#[tokio::test]
async fn a_copy_of_chosen_files_refuses_a_file_nobody_chose() {
    let dir = TempDir::new();
    let files = dir.join("checkout/files");
    std::fs::create_dir_all(files.join(".claude")).unwrap();
    std::fs::write(files.join(".env"), "TOKEN=1").unwrap();
    std::fs::write(files.join(".claude/settings.json"), "{}").unwrap();
    let walked = walk(&files).unwrap();
    let (_rig, handle, _events, mut device) = linked(&dir).await;
    let workspace = WorkspaceId::derive("w");

    let (copied, transfer) = tokio::join!(
        // It stops at the manifest, so nothing here sends the bytes.
        tokio::time::timeout(
            Duration::from_secs(10),
            handle.copy_files(workspace, vec![".env".to_owned()], |_| {}),
        ),
        async {
            let ClientMessage::FetchFiles { transfer, .. } = device.message().await else {
                panic!("expected a fetch of files");
            };
            device
                .send(&ServerMessage::FetchManifest {
                    transfer,
                    entries: walked.entries.clone(),
                })
                .await;
            transfer
        }
    );

    let Err(TransferError::Failed { code, message }) = copied.expect("it refuses at once") else {
        panic!("the copy should be refused");
    };
    assert_eq!(code, ErrorCode::BadPath);
    assert!(message.contains("wasn't chosen"), "{message}");
    assert!(!dir.join("staging").join(transfer.to_string()).exists());
}

/// Answers the next fetch with `walked`'s manifest and gives its key and
/// transfer. Credit left over from one before is passed by.
async fn manifest_for(
    device: &mut Device,
    walked: &Walk,
    path: &str,
) -> (IdempotencyKey, TransferId) {
    loop {
        match device.message().await {
            ClientMessage::FetchBegin {
                key,
                transfer,
                path: asked,
                ..
            } => {
                assert_eq!(asked, path);
                device
                    .send(&ServerMessage::FetchManifest {
                        transfer,
                        entries: walked.entries.clone(),
                    })
                    .await;
                return (key, transfer);
            }
            ClientMessage::TransferCredit { .. } => {}
            other => panic!("expected a fetch, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_picture_lands_in_the_cache_under_its_hash_then_comes_from_there() {
    let dir = TempDir::new();
    let shared = dir.join("agent");
    std::fs::create_dir_all(&shared).unwrap();
    let bytes = pattern(200 * 1024);
    std::fs::write(shared.join("shot.PNG"), &bytes).unwrap();
    let walked = walk(&shared.join("shot.PNG")).unwrap();
    let sha = walked.entries[0].sha256.clone().unwrap();
    let (_rig, handle, _events, mut device) = linked(&dir).await;

    let (got, ()) = tokio::join!(
        handle.media(agent(), "../../shot.PNG".into(), 1024 * 1024, false, |_| {}),
        async {
            let (key, transfer) = manifest_for(&mut device, &walked, "../../shot.PNG").await;
            device.serve(transfer, &walked).await;
            device
                .send(&ServerMessage::ActionResult {
                    key,
                    outcome: ActionOutcome::Done,
                })
                .await;
        }
    );

    let name = format!("{sha}.png");
    assert_eq!(
        got,
        Ok(Media::Cached {
            name: name.clone(),
            size: bytes.len() as u64,
        })
    );
    assert_eq!(std::fs::read(dir.join("media").join(&name)).unwrap(), bytes);
    assert!(!dir.join("Downloads/MaxHQ").join("shot.PNG").exists());

    // Asked again, the manifest says it's the same file, so nothing more
    // comes and the device hears it can stop.
    let (again, ()) = tokio::join!(
        handle.media(agent(), "../../shot.PNG".into(), 0, false, |_| {}),
        async {
            let (_, transfer) = manifest_for(&mut device, &walked, "../../shot.PNG").await;
            assert_eq!(
                device.message().await,
                ClientMessage::UploadCancel { transfer }
            );
        }
    );
    assert_eq!(
        again,
        Ok(Media::Cached {
            name,
            size: bytes.len() as u64,
        })
    );
}

#[tokio::test]
async fn a_picture_the_system_cleared_from_the_cache_comes_again() {
    let dir = TempDir::new();
    let shared = dir.join("agent");
    std::fs::create_dir_all(&shared).unwrap();
    let bytes = pattern(64 * 1024);
    std::fs::write(shared.join("shot.png"), &bytes).unwrap();
    let walked = walk(&shared.join("shot.png")).unwrap();
    let sha = walked.entries[0].sha256.clone().unwrap();
    let (_rig, handle, _events, mut device) = linked(&dir).await;
    let cached = Ok(Media::Cached {
        name: format!("{sha}.png"),
        size: bytes.len() as u64,
    });

    // A phone's system may empty its caches, folder and all, between one
    // look and the next. Each look finds the picture all the same.
    for _ in 0..2 {
        let (got, ()) = tokio::join!(
            handle.media(agent(), "shot.png".into(), 1024 * 1024, false, |_| {}),
            async {
                let (key, transfer) = manifest_for(&mut device, &walked, "shot.png").await;
                device.serve(transfer, &walked).await;
                device
                    .send(&ServerMessage::ActionResult {
                        key,
                        outcome: ActionOutcome::Done,
                    })
                    .await;
            }
        );
        assert_eq!(got, cached);
        let kept = dir.join("media").join(format!("{sha}.png"));
        assert_eq!(std::fs::read(kept).unwrap(), bytes);
        std::fs::remove_dir_all(dir.join("media")).unwrap();
    }
}

#[tokio::test]
async fn a_video_past_the_limit_stays_on_the_device_and_says_its_size() {
    let dir = TempDir::new();
    let shared = dir.join("agent");
    std::fs::create_dir_all(&shared).unwrap();
    let bytes = pattern(300 * 1024);
    std::fs::write(shared.join("clip.mp4"), &bytes).unwrap();
    let walked = walk(&shared.join("clip.mp4")).unwrap();
    let (_rig, handle, _events, mut device) = linked(&dir).await;

    let (got, ()) = tokio::join!(
        handle.media(agent(), "/work/clip.mp4".into(), 0, false, |_| {}),
        async {
            let (_, transfer) = manifest_for(&mut device, &walked, "/work/clip.mp4").await;
            assert_eq!(
                device.message().await,
                ClientMessage::UploadCancel { transfer }
            );
        }
    );

    assert_eq!(
        got,
        Ok(Media::TooLarge {
            size: bytes.len() as u64
        })
    );
    let left: Vec<_> = std::fs::read_dir(dir.join("media"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|item| item.file_name() != ".incoming")
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
async fn media_the_cache_doesnt_keep_is_refused_before_it_goes() {
    let dir = TempDir::new();
    let shared = dir.join("agent");
    std::fs::create_dir_all(shared.join("shots.png")).unwrap();
    std::fs::write(shared.join("shots.png/a.png"), pattern(10)).unwrap();
    let folder = walk(&shared.join("shots.png")).unwrap();
    let (_rig, handle, _events, mut device) = linked(&dir).await;

    let page = handle
        .media(agent(), "/work/setup.exe".into(), 1024, false, |_| {})
        .await;
    assert!(matches!(
        page,
        Err(TransferError::Failed {
            code: ErrorCode::NotAllowed,
            ..
        })
    ));
    handle.send(mark_seen()).await.unwrap();
    assert_eq!(device.message().await, mark_seen());

    // A folder named like a picture isn't one.
    let (got, ()) = tokio::join!(
        handle.media(agent(), "/work/shots.png".into(), 1024, false, |_| {}),
        async {
            let (_, transfer) = manifest_for(&mut device, &folder, "/work/shots.png").await;
            assert_eq!(
                device.message().await,
                ClientMessage::UploadCancel { transfer }
            );
        }
    );
    assert!(matches!(
        got,
        Err(TransferError::Failed {
            code: ErrorCode::NotAllowed,
            ..
        })
    ));
}

/// The next fetch the device hears, as its key, transfer, path and
/// whether it's for Show anyway.
async fn fetch_asked(device: &mut Device) -> (IdempotencyKey, TransferId, String, bool) {
    loop {
        match device.message().await {
            ClientMessage::FetchBegin {
                key,
                transfer,
                path,
                anyway,
                ..
            } => return (key, transfer, path, anyway),
            ClientMessage::TransferCredit { .. } => {}
            other => panic!("expected a fetch, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_path_outside_the_agents_folders_comes_back_refused_then_shows_anyway() {
    let dir = TempDir::new();
    let shared = dir.join("elsewhere");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(shared.join("plan.md"), pattern(64)).unwrap();
    let walked = walk(&shared.join("plan.md")).unwrap();
    let (_rig, handle, _events, mut device) = linked(&dir).await;

    let (got, ()) = tokio::join!(
        handle.media(agent(), "/tmp/example/plan.md".into(), 1024, false, |_| {}),
        async {
            let (_, transfer, _, anyway) = fetch_asked(&mut device).await;
            assert!(!anyway);
            device
                .failed(
                    transfer,
                    ErrorCode::Outside,
                    "the path is outside the agent's folders",
                )
                .await;
        }
    );
    assert_eq!(
        got,
        Ok(Media::Refused {
            outside: true,
            message: "the path is outside the agent's folders".into(),
        })
    );

    // A bad path is refused too, but Show anyway can't help with it.
    let (bad, ()) = tokio::join!(
        handle.media(agent(), "/tmp/example/gone.md".into(), 1024, false, |_| {}),
        async {
            let (_, transfer, _, _) = fetch_asked(&mut device).await;
            device
                .failed(transfer, ErrorCode::BadPath, "no such file")
                .await;
        }
    );
    assert_eq!(
        bad,
        Ok(Media::Refused {
            outside: false,
            message: "no such file".into(),
        })
    );

    let (shown, ()) = tokio::join!(
        handle.media(agent(), "/tmp/example/plan.md".into(), 1024, true, |_| {}),
        async {
            let (key, transfer, path, anyway) = fetch_asked(&mut device).await;
            assert_eq!((path.as_str(), anyway), ("/tmp/example/plan.md", true));
            device
                .send(&ServerMessage::FetchManifest {
                    transfer,
                    entries: walked.entries.clone(),
                })
                .await;
            device.serve(transfer, &walked).await;
            device
                .send(&ServerMessage::ActionResult {
                    key,
                    outcome: ActionOutcome::Done,
                })
                .await;
        }
    );
    assert!(
        matches!(shown, Ok(Media::Cached { size: 64, .. })),
        "{shown:?}"
    );
}

#[tokio::test]
async fn show_anyway_never_goes_to_a_maxd_from_before_it() {
    let dir = TempDir::new();
    let mut rig = rig(&dir);
    let (handle, mut events) = rig.client.link(spec(Route::Local));
    let older = ProtocolRange {
        current: SHOW_ANYWAY_SINCE - 1,
        oldest: OLDEST_PROTOCOL_VERSION,
    };
    let mut device = Device::speaking(&mut rig, older).await;
    while !is_connected(&next_state(&mut events).await) {}

    let got = handle
        .media(agent(), "/tmp/example/plan.md".into(), 1024, true, |_| {})
        .await;
    assert_eq!(got, Err(TransferError::Link(LinkError::TooOld)));
    handle.send(mark_seen()).await.unwrap();
    assert_eq!(device.message().await, mark_seen());
}

#[tokio::test]
async fn requests_overtake_file_data_on_the_way_out() {
    let dir = TempDir::new();
    let total = 8 * 1024 * 1024;
    let (_rig, handle, _events, mut device) = linked(&dir).await;
    let staged = attachment("clip.png", AttachmentKind::Image, total as u64);

    let (result, ()) = tokio::join!(
        handle.upload_bytes(
            UploadTarget::Device,
            "clip".into(),
            "image/png",
            pattern(total),
            |_| {},
        ),
        async {
            let begin = upload_begin(device.message().await);
            assert_eq!(begin.name, "clip.png");
            device.credit(begin.transfer, (0, total as u64)).await;
            // File data backs up behind the unread socket meanwhile.
            tokio::time::sleep(Duration::from_millis(300)).await;
            handle.send(mark_seen()).await.unwrap();
            let mut before = 0;
            loop {
                match device.got().await {
                    Got::Chunk { data, .. } => before += data.len(),
                    Got::Message(message) => {
                        assert_eq!(message, mark_seen());
                        break;
                    }
                }
            }
            assert!(before < total / 2, "{before} bytes came before the request");
            device.skip(begin.transfer, (total - before) as u64).await;
            device.staged(&begin, &staged).await;
        }
    );

    assert_eq!(result, Ok(ActionOutcome::Staged { attachment: staged }));
}

#[tokio::test]
async fn a_ping_gives_how_long_its_pong_took() {
    let dir = TempDir::new();
    let (_rig, handle, _events, mut device) = linked(&dir).await;
    assert!(handle.capabilities().transfers);

    let (took, ()) = tokio::join!(handle.ping(), async {
        let ClientMessage::Ping { nonce } = device.message().await else {
            panic!("expected a ping");
        };
        device.send(&ServerMessage::Pong { nonce }).await;
    });

    assert!(took.is_ok());
}

#[tokio::test]
async fn transfers_need_a_device_that_does_them() {
    let dir = TempDir::new();
    let mut rig = rig(&dir);
    let (handle, _events, _maxd) = connected(&mut rig, InstanceId::generate()).await;
    assert_eq!(handle.capabilities(), Capabilities::default());

    let upload = handle
        .upload_bytes(UploadTarget::Device, "a".into(), "", vec![1], |_| {})
        .await;
    let download = handle.download(agent(), "a".into(), |_| {}).await;

    assert_eq!(upload, Err(TransferError::Unsupported));
    assert_eq!(download, Err(TransferError::Unsupported));
}
