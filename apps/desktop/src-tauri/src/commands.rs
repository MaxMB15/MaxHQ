// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The commands the webview can call besides the native ones. Each one is
//! also listed in [`crate::specta_builder`], which is what generates its
//! TypeScript. Everything that answers once goes through [`request`], and
//! the rest stream back through a channel.

use std::marker::PhantomData;
use std::str::FromStr;
use std::sync::{Arc, OnceLock};

use maxhq_app::fleet::{BinaryDirs, FleetProgress};
use maxhq_app::request::CommandError;
use maxhq_app::setup::{SetupProgress, SetupSecret};
use maxhq_app::timeline::{Sink, TimelineMessage};
use maxhq_app::transfers::{
    CopiedProject, CopyTarget, Downloaded, MediaFile, Report, TransferProgress,
};
use maxhq_app::workflow::WorkflowMessage;
use maxhq_app::{App, Caller, Request, Response};
use maxhq_protocol::ids::{AgentId, DeviceId, LinkId, WorkspaceId};
use maxhq_protocol::message::{ActionOutcome, UploadTarget};
use maxhq_protocol::workflow::RunKey;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use specta::datatype::DataType;
use tauri::ipc::{Channel, JavaScriptChannelId};
use tauri::{AppHandle, Manager, State, Webview};

/// Answers one request from this window with a response of its kind.
#[tauri::command]
#[specta::specta]
pub async fn request(
    webview: Webview,
    app: State<'_, App>,
    request: Request,
) -> Result<Response, CommandError> {
    let on_screen = match request {
        // Asking the window takes a turn of the main thread, and only what
        // its panes show depends on the answer.
        Request::SetShown { .. } => on_screen(&webview),
        _ => true,
    };
    let caller = Caller {
        window: webview.label().to_owned(),
        on_screen,
    };
    app.request(&caller, request).await
}

/// Whether the webview's window is on screen. A window that's minimized or
/// hidden shows nothing, whatever its page says. When the window can't
/// tell, the page's report stands.
fn on_screen(webview: &Webview) -> bool {
    let window = webview.window();
    window.is_visible().unwrap_or(true) && !window.is_minimized().unwrap_or(false)
}

/// Opens an agent's timeline in this window. Updates come through
/// `channel`, the window's one timeline channel, starting with a reset to
/// the newest items. Each open needs a close, though a window that reloads
/// or closes lets go of all its opens.
#[tauri::command]
#[specta::specta]
pub async fn open_timeline(
    webview: Webview,
    app: State<'_, App>,
    agent: AgentId,
    channel: TimelineChannelName,
) -> Result<(), CommandError> {
    let label = webview.label().to_owned();
    let sink = Arc::new(WindowChannel::new(channel, webview));
    app.open_timeline(&label, sink, agent).await
}

/// Opens a workflow run in this window. Updates come through `channel`,
/// the window's one workflow channel, starting with a reset. Each open
/// needs a `close_workflow` request, though a window that reloads or closes
/// lets go of all its opens.
#[tauri::command]
#[specta::specta]
pub async fn open_workflow(
    webview: Webview,
    app: State<'_, App>,
    run: RunKey,
    channel: ChannelName<WorkflowMessage>,
) -> Result<(), CommandError> {
    let label = webview.label().to_owned();
    let sink = Arc::new(WindowChannel::new(channel, webview));
    app.open_workflow(&label, sink, run).await
}

/// The window's timeline channel as the page names it.
pub type TimelineChannelName = ChannelName<TimelineMessage>;

/// One of the window's channels as the page names it. The page sends a
/// `Channel`, and the TypeScript says so, but every open names the same
/// one and the app keeps only the first open's.
pub struct ChannelName<M> {
    pub(crate) id: u32,
    pub(crate) name: JavaScriptChannelId,
    message: PhantomData<fn() -> M>,
}

impl<'de, M> Deserialize<'de> for ChannelName<M> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let name = JavaScriptChannelId::from_str(&text).map_err(D::Error::custom)?;
        // The name checked out, so the id is the number after its prefix.
        let id = text
            .rsplit(':')
            .next()
            .and_then(|id| id.parse().ok())
            .ok_or_else(|| D::Error::custom("invalid channel ID"))?;
        Ok(Self {
            id,
            name,
            message: PhantomData,
        })
    }
}

impl<M: specta::Type> specta::Type for ChannelName<M> {
    fn definition(types: &mut specta::Types) -> DataType {
        <Channel<Vec<M>> as specta::Type>::definition(types)
    }
}

/// One of a window's channels, as the app sends batches down it. Tauri's
/// half of a channel tells the page it has ended when it goes, and a page
/// told that before its first batch stops listening, for everything it
/// has open there. So the half is only made for the first batch, which
/// only the open the app kept sends, and the opens it drops never had one.
struct WindowChannel<M> {
    name: ChannelName<M>,
    webview: Webview,
    channel: OnceLock<Channel<Vec<M>>>,
}

impl<M> WindowChannel<M> {
    fn new(name: ChannelName<M>, webview: Webview) -> Self {
        Self {
            name,
            webview,
            channel: OnceLock::new(),
        }
    }
}

impl<M: Serialize + Clone + Send + Sync + 'static> Sink<M> for WindowChannel<M> {
    fn id(&self) -> u32 {
        self.name.id
    }

    fn send(&self, batch: Vec<M>) -> bool {
        self.channel
            .get_or_init(|| self.name.name.channel_on(self.webview.clone()))
            .send(batch)
            .is_ok()
    }
}

/// Uploads a file or folder you handed over to `target` on a device, and
/// gives what the device made of it. `progress` hears how far it has got.
#[tauri::command]
#[specta::specta]
pub async fn upload(
    app: State<'_, App>,
    device: DeviceId,
    target: UploadTarget,
    path: String,
    progress: Channel<TransferProgress>,
) -> Result<ActionOutcome, CommandError> {
    app.upload(device, target, path, reporter(progress)).await
}

/// Uploads pasted bytes to `target` on a device as one file called `name`.
/// `data` is the bytes in base64.
#[tauri::command]
#[specta::specta]
pub async fn upload_pasted(
    app: State<'_, App>,
    device: DeviceId,
    target: UploadTarget,
    name: String,
    mime: String,
    data: String,
    progress: Channel<TransferProgress>,
) -> Result<ActionOutcome, CommandError> {
    app.upload_pasted(device, target, name, mime, data, reporter(progress))
        .await
}

/// Updates `devices`, or every device when it's empty, to the `maxd` this
/// app carries, and their agent CLIs too when `tools` is set. `unsigned`
/// says you agreed, in the window's dialog, to send a `maxd` no release
/// signed, built on this Mac. `progress` hears each device's every step.
#[tauri::command]
#[specta::specta]
pub async fn update_fleet(
    app: State<'_, App>,
    devices: Vec<DeviceId>,
    tools: bool,
    unsigned: bool,
    progress: Channel<FleetProgress>,
) -> Result<(), CommandError> {
    let binaries = BinaryDirs::for_app();
    let report = move |step| {
        let _ = progress.send(step);
    };
    app.update_fleet(devices, tools, unsigned, &binaries, &report)
        .await;
    Ok(())
}

/// Sets `host` up as a device over your own SSH login, then hands it the
/// logins in `secrets` over its link. `progress` hears each step, ending
/// with done or failed.
#[tauri::command]
#[specta::specta]
pub async fn setup_device(
    app: State<'_, App>,
    host: String,
    secrets: Vec<SetupSecret>,
    progress: Channel<SetupProgress>,
) -> Result<LinkId, CommandError> {
    let report = move |step| {
        let _ = progress.send(step);
    };
    app.setup_device(host, secrets, &report).await
}

/// Fetches a file or folder an agent can see into `~/Downloads/MaxHQ`,
/// under a name nothing there has yet. `progress` hears how far it has
/// got.
#[tauri::command]
#[specta::specta]
pub async fn download(
    app: State<'_, App>,
    agent: AgentId,
    path: String,
    progress: Channel<TransferProgress>,
) -> Result<Downloaded, CommandError> {
    app.download(agent, path, reporter(progress)).await
}

/// Readies a file or folder an agent can see, such as a picture in its
/// timeline, for an agent on `device`. It comes through the agent's device
/// to this machine, then uploads to `target` when one is given and stages
/// on `device` otherwise. `progress` hears both legs.
#[tauri::command]
#[specta::specta]
pub async fn attach_from_agent(
    app: State<'_, App>,
    device: DeviceId,
    target: Option<UploadTarget>,
    agent: AgentId,
    path: String,
    progress: Channel<TransferProgress>,
) -> Result<ActionOutcome, CommandError> {
    app.attach_from_agent(device, target, agent, path, reporter(progress))
        .await
}

/// Copies a workspace's project to `target`: a new container or a clone in
/// a folder. Once it is ready, the chosen `files` come over from the source
/// checkout and land beside what git tracks. `progress` hears the files
/// move, and the answer names the new workspace.
#[tauri::command]
#[specta::specta]
pub async fn copy_project(
    app: State<'_, App>,
    from: WorkspaceId,
    target: CopyTarget,
    files: Vec<String>,
    progress: Channel<TransferProgress>,
) -> Result<CopiedProject, CommandError> {
    app.copy_project(from, target, files, reporter(progress))
        .await
}

/// Fetches a picture or video an agent can see into the media cache, for
/// the webview to load through the media scheme. One larger than `limit`
/// bytes stays on the device, and the answer gives its size. `anyway`
/// asks for one file outside the agent's folders, from your click on Show
/// anyway. `progress` hears how far it has got.
#[tauri::command]
#[specta::specta]
pub async fn media(
    app: State<'_, App>,
    agent: AgentId,
    path: String,
    limit: u32,
    anyway: bool,
    progress: Channel<TransferProgress>,
) -> Result<MediaFile, CommandError> {
    app.media(agent, path, limit, anyway, reporter(progress))
        .await
}

/// Opens Terminal on this Mac with the root fix of `item` in `device`'s
/// host report: the command alone for this Mac, or over `ssh -t` for
/// another device. The command comes from the device's report, so the
/// page can't have Terminal run anything else.
#[tauri::command]
#[specta::specta]
pub async fn open_root_fix(
    handle: AppHandle,
    app: State<'_, App>,
    device: DeviceId,
    item: String,
) -> Result<(), CommandError> {
    let line = app.root_fix_line(device, &item)?;
    let dir = handle
        .path()
        .app_cache_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("maxhq"))
        .join("terminal");
    crate::terminal::open(dir, line)
        .await
        .map_err(|message| CommandError::Failed { message })
}

/// Sends each step of a transfer down its channel. A window that went away
/// misses the rest, and the transfer goes on.
fn reporter(channel: Channel<TransferProgress>) -> impl Report {
    move |progress| {
        let _ = channel.send(progress);
    }
}
