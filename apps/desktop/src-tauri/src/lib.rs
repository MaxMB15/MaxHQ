// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The MaxHQ desktop app: the Tauri shell around [`maxhq_app`], which
//! connects to `maxd` on each device you add and merges what they report
//! into one view. The shell hands that view to the webview, carries the
//! webview's requests in, and does what only macOS can.

pub mod bindings;
mod commands;
mod native;
mod notify;
mod screen;
mod terminal;
mod terminal_stream;
mod updater;

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use maxhq_app::app_keys;
use maxhq_app::app_update::UpdatePrefs;
use maxhq_app::devices::{self, Devices};
use maxhq_app::editors;
use maxhq_app::media::{self, Served};
use maxhq_app::notify::NotificationPrefs;
use maxhq_app::registry::Registry;
use maxhq_app::screen::ScreenOn;
use maxhq_app::view::{Emit, View};
use maxhq_app::viewer;
use maxhq_app::{App, Opening, Shell};
use maxhq_protocol::ids::AgentId;
use maxhq_protocol::state::Delta;
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Typescript;
use tauri::http;
use tauri::webview::{PageLoadEvent, PageLoadPayload};
use tauri::{
    AppHandle, Manager, UriSchemeContext, UriSchemeResponder, Webview, Window, WindowEvent,
};
use tauri_specta::{collect_commands, collect_events, Builder, Event};

/// Sent with the view's changes, oldest first, at most once a frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, Event)]
pub struct ViewChanged(pub Vec<Delta>);

/// Sent when you click a notification, so the webview opens that agent's
/// pane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, Event)]
pub struct FocusAgent {
    /// The agent the notification was about.
    pub agent: AgentId,
}

/// Lists every command and every protocol type the webview sees. The app
/// registers its commands from this, and the bindings test exports from it,
/// so the TypeScript can't drift from what the app actually serves.
pub fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        // One TypeScript type per Rust type. With phases on, every struct
        // holding an `Option` also gets a second type for the shape serde
        // accepts, where the field may be missing. The webview reads what
        // Rust writes and always sends whole messages, so it never needs
        // the looser one.
        .disable_serde_phases()
        .commands(collect_commands![
            commands::request,
            commands::open_timeline,
            commands::open_workflow,
            commands::upload,
            commands::upload_pasted,
            commands::download,
            commands::attach_from_agent,
            commands::copy_project,
            commands::media,
            commands::update_fleet,
            commands::setup_device,
            commands::open_root_fix,
            commands::mask_output,
            native::check_spelling,
            native::spelling_menu,
            native::pick_attachments,
            native::read_clipboard_files,
            native::describe_paths,
            native::trash_offered,
            native::reveal_download,
            native::save_export,
            native::pick_path,
            notify::notification_prefs,
            notify::set_notification_prefs,
            notify::notification_permission,
            notify::send_test_notification,
            notify::open_notification_settings,
            screen::screen_on,
            screen::set_screen_on,
            updater::app_update,
            updater::check_for_update,
            updater::install_update,
            updater::set_update_prefs,
        ])
        .events(collect_events![
            ViewChanged,
            FocusAgent,
            updater::AppUpdateChanged
        ])
        .typ::<maxhq_protocol::ClientMessage>()
        .typ::<maxhq_protocol::ServerMessage>()
        .constant("PROTOCOL_VERSION", maxhq_protocol::PROTOCOL_VERSION)
        .constant(
            "OLDEST_PROTOCOL_VERSION",
            maxhq_protocol::OLDEST_PROTOCOL_VERSION,
        )
        .constant(
            "MAX_INLINE_ATTACHMENT",
            maxhq_protocol::message::MAX_INLINE_ATTACHMENT,
        )
        .constant("DEPTH", maxhq_protocol::agents::DEPTH)
        .constant("STRUCTURAL_KEYS", maxhq_protocol::tasks::STRUCTURAL_KEYS)
        .constant(
            "STRUCTURAL_SUFFIXES",
            maxhq_protocol::tasks::STRUCTURAL_SUFFIXES,
        )
        .constant("MAX_NOTE_TITLE", maxhq_protocol::notes::MAX_NOTE_TITLE)
        .constant("MAX_NOTE_BODY", maxhq_protocol::notes::MAX_NOTE_BODY)
        .constant(
            "NOTE_PILLS_SINCE",
            maxhq_protocol::message::NOTE_PILLS_SINCE,
        )
        .constant(
            "SHOW_ANYWAY_SINCE",
            maxhq_protocol::message::SHOW_ANYWAY_SINCE,
        )
        .constant(
            "ANSWER_PILLS_SINCE",
            maxhq_protocol::message::ANSWER_PILLS_SINCE,
        )
        .constant("CLI_LOGIN_SINCE", maxhq_protocol::message::CLI_LOGIN_SINCE)
        .constant(
            "CURSOR_LOGIN_SINCE",
            maxhq_protocol::message::CURSOR_LOGIN_SINCE,
        )
        .constant("VISUALS_SINCE", maxhq_protocol::message::VISUALS_SINCE)
        .constant("EFFORT_SINCE", maxhq_protocol::message::EFFORT_SINCE)
        .constant("TAKE_OVER_SINCE", maxhq_protocol::message::TAKE_OVER_SINCE)
        .constant("DISMISS_SINCE", maxhq_protocol::message::DISMISS_SINCE)
        .constant(
            "SIGNED_UPDATES_SINCE",
            maxhq_protocol::message::SIGNED_UPDATES_SINCE,
        )
        .constant(
            "MOVE_AGENT_SINCE",
            maxhq_protocol::message::MOVE_AGENT_SINCE,
        )
        .constant(
            "FINISHED_SUB_AGENTS_SINCE",
            maxhq_protocol::message::FINISHED_SUB_AGENTS_SINCE,
        )
        .constant("TERMINALS_SINCE", maxhq_protocol::message::TERMINALS_SINCE)
        .constant("FOLDERS_SINCE", maxhq_protocol::message::FOLDERS_SINCE)
        .constant("SESSIONS_SINCE", maxhq_protocol::message::SESSIONS_SINCE)
        .constant(
            "DISPATCHER_CHATS_SINCE",
            maxhq_protocol::message::DISPATCHER_CHATS_SINCE,
        )
}

/// The exporter settings for the generated file.
pub fn typescript() -> Typescript {
    Typescript::default()
        .header("// Regenerate with `pnpm bindings` after changing the Rust types.")
}

/// Starts the app and blocks until its last window closes.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let builder = specta_builder();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(native::invoke_handler(builder.invoke_handler()))
        .on_window_event(|window, event| {
            native::on_window_event(window, event);
            forget_closed(window, event);
        })
        .on_page_load(release_reloaded)
        .register_asynchronous_uri_scheme_protocol(media::SCHEME, serve_media)
        .register_uri_scheme_protocol(viewer::SCHEME, serve_viewer)
        .setup(move |app| {
            builder.mount_events(app);
            let runtime = tauri::async_runtime::handle().inner().clone();
            let handle = app.handle().clone();
            let emit: Emit = Box::new(move |deltas| {
                if let Err(error) = ViewChanged(deltas).emit(&handle) {
                    tracing::warn!(%error, "couldn't send a view change to the webview");
                }
            });
            let registry = open_registry(app.handle())?;
            let handle = app.handle().clone();
            let (notifications, sender) = notify::Notifications::start(
                move |agent| focus_agent(&handle, agent),
                NotificationPrefs::load(&registry),
            );
            app.manage(notifications);
            app.manage(updater::Updates::new(UpdatePrefs::load(&registry)));
            app.manage(ScreenOn::start(&registry));
            let view = View::new(emit, sender);
            let downloads = app.path().download_dir().ok();
            let media = media_folder(app.handle());
            let mut client = devices::client(
                &runtime,
                downloads.map(|dir| dir.join(native::DOWNLOADS_FOLDER)),
                Some(media.clone()),
            );
            app.manage(MediaFolder(media));
            // The key goes in before the first link opens, so every link
            // proves it.
            let app_key = app_keys::from_keychain(&registry);
            if let Some(keys) = &app_key {
                client = client.proving(Arc::new(keys.clone()));
            }
            let devices = Devices::start(
                runtime,
                client,
                registry,
                view.clone(),
                devices::bundled_maxd(),
            );
            if let Some(keys) = app_key {
                devices.keep_app_key(keys);
            }
            keep_ssh(app.handle(), &devices);
            devices.link_peers(devices::SshFacts);
            let desktop = Desktop {
                app: app.handle().clone(),
            };
            app.manage(App::new(view, devices, desktop));
            keep_local_maxd_current(app.handle());
            updater::start(app.handle());
            open_main_window(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("the Tauri runtime failed to start");
}

/// Restarts this Mac's maxd once it says hello, when its login service
/// runs the maxd inside this app and that one is a different build: the
/// app was updated or put back in place, and the maxd still running is
/// the one from before.
fn keep_local_maxd_current(handle: &AppHandle) {
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        handle.state::<App>().keep_local_maxd_current().await;
    });
}

/// Makes the main window from its config, once the app's state is there
/// for its first requests. A link or `window.open` that asks for a new
/// window opens in the browser instead, since the webview alone does
/// nothing with it.
fn open_main_window(app: &tauri::App) -> tauri::Result<()> {
    let Some(config) = app.config().app.windows.iter().find(|w| w.label == "main") else {
        return Ok(());
    };
    let desktop = Desktop {
        app: app.handle().clone(),
    };
    tauri::WebviewWindowBuilder::from_config(app.handle(), config)?
        .on_new_window(move |url, _| {
            if editors::page_link_allowed(url.as_str()) {
                let opening = desktop.open_url(url.to_string());
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = opening.await {
                        tracing::warn!(%error, "couldn't open a link in the browser");
                    }
                });
            } else {
                tracing::warn!(
                    scheme = url.scheme(),
                    "refused a page's link that isn't to the web"
                );
            }
            tauri::webview::NewWindowResponse::Deny
        })
        .build()?;
    Ok(())
}

/// Keeps MaxHQ's SSH files, which editors reach container workspaces
/// through, in the `ssh` folder of the app's data folder. Without them the
/// app runs on, and no editor opens a container.
fn keep_ssh<C: devices::Connector>(app: &AppHandle, devices: &Devices<C>) {
    let path = app.path();
    let kept = match (path.app_data_dir(), path.home_dir()) {
        (Ok(data), Ok(home)) => devices
            .keep_ssh(data.join("ssh"), home)
            .map_err(|error| error.to_string()),
        (Err(error), _) | (_, Err(error)) => Err(error.to_string()),
    };
    if let Err(error) = kept {
        tracing::error!(%error, "couldn't set up MaxHQ's SSH files, so editors can't open containers");
    }
}

/// Opens the device list in the app's data folder. If that fails, the app
/// still runs on a list kept in memory, so this machine shows and anything
/// you add lasts until you quit.
fn open_registry(app: &AppHandle) -> Result<Registry, maxhq_core::db::OpenError> {
    let opened = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())
        .and_then(|dir| {
            std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
            Registry::open(&dir.join("maxhq.db")).map_err(|error| error.to_string())
        });
    match opened {
        Ok(registry) => Ok(registry),
        Err(error) => {
            tracing::error!(%error, "couldn't open the device list, so it won't be saved");
            Registry::in_memory()
        }
    }
}

/// Where the app keeps pictures and videos the timeline shows.
struct MediaFolder(PathBuf);

/// The media cache, in the app's cache folder.
fn media_folder(app: &AppHandle) -> PathBuf {
    app.path()
        .app_cache_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("maxhq"))
        .join("media")
}

/// Answers the webview's requests for cached pictures and videos. Reading
/// a file takes a blocking thread, so the webview's own thread never waits
/// on the disk.
fn serve_media(
    ctx: UriSchemeContext<'_, tauri::Wry>,
    request: http::Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let Some(folder) = ctx
        .app_handle()
        .try_state::<MediaFolder>()
        .map(|folder| folder.0.clone())
    else {
        // The app hasn't finished starting, so there's no cache yet.
        responder.respond(media_response(Served {
            status: 503,
            headers: Vec::new(),
            body: Vec::new(),
        }));
        return;
    };
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    let range = request
        .headers()
        .get(http::header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let origin = request
        .headers()
        .get(http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    tauri::async_runtime::spawn_blocking(move || {
        let served = media::serve_to(&folder, &method, &path, range.as_deref(), origin.as_deref());
        responder.respond(media_response(served));
    });
}

/// Answers the viewer frame's requests from the app's bundled files.
fn serve_viewer(
    ctx: UriSchemeContext<'_, tauri::Wry>,
    request: http::Request<Vec<u8>>,
) -> http::Response<Vec<u8>> {
    let resolver = ctx.app_handle().asset_resolver();
    let uri = request.uri();
    let served = viewer::serve(request.method().as_str(), uri.path(), uri.query(), |path| {
        resolver.get(path.to_owned()).map(|asset| viewer::Bundled {
            bytes: asset.bytes,
            mime: asset.mime_type,
        })
    });
    media_response(served)
}

/// The HTTP answer for what the cache or the viewer served.
fn media_response(served: Served) -> http::Response<Vec<u8>> {
    let mut response = http::Response::builder().status(served.status);
    for (name, value) in served.headers {
        response = response.header(name, value);
    }
    response.body(served.body).unwrap_or_else(|error| {
        tracing::warn!(%error, "couldn't build an answer from the media cache");
        let mut failed = http::Response::new(Vec::new());
        *failed.status_mut() = http::StatusCode::INTERNAL_SERVER_ERROR;
        failed
    })
}

/// Forgets what a window's panes showed once the window closes, so the
/// agents it showed alert again, and lets go of the timelines it watched.
/// Each window holds one webview under the window's label.
fn forget_closed(window: &Window, event: &WindowEvent) {
    if !matches!(event, WindowEvent::Destroyed) {
        return;
    }
    if let Some(app) = window.try_state::<App>() {
        app.close_window(window.label());
    }
}

/// Lets go of the timelines a page watched once its webview starts loading
/// a new one, such as on a reload. The new page's scripts haven't run yet,
/// so what it opens comes after.
fn release_reloaded(webview: &Webview, payload: &PageLoadPayload<'_>) {
    if matches!(payload.event(), PageLoadEvent::Started)
        && let Some(app) = webview.try_state::<App>()
    {
        app.release_window(webview.label());
    }
}

/// Brings the window forward and opens an agent's pane, for a click on a
/// notification about it.
fn focus_agent(app: &AppHandle, agent: AgentId) {
    if let Some(window) = app.get_webview_window("main") {
        // Each step can fail only once the window is closing, when there's
        // nothing to bring forward.
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    if let Err(error) = (FocusAgent { agent }).emit(app) {
        tracing::warn!(%error, "couldn't ask the webview to focus an agent");
    }
}

/// How long the app waits to quit after it deletes its data, so the
/// answer reaches the window and the webview clears what it kept.
const QUIT_AFTER: Duration = Duration::from_secs(1);

/// What only the desktop can do for the app.
struct Desktop {
    app: AppHandle,
}

impl Shell for Desktop {
    fn open_browser(&self, url: String) -> Opening {
        Box::pin(open(url, "The browser"))
    }

    fn open_url(&self, url: String) -> Opening {
        if !editors::url_allowed(&url) {
            return Box::pin(async { Err("MaxHQ only opens web and editor links".to_owned()) });
        }
        Box::pin(open(url, "The link"))
    }

    fn spawn_editor(&self, program: PathBuf, args: Vec<String>) -> Opening {
        Box::pin(spawn_editor(program, args))
    }

    /// The app's data and cache folders, and the MaxHQ folder it shares
    /// with maxd, where the app's links keep their known hosts. maxd has
    /// deleted the rest of that one by the time this runs. Each one goes
    /// only when it's named for the app, so an odd path from the system
    /// can't take anything else with it.
    fn app_data(&self) -> Vec<PathBuf> {
        let path = self.app.path();
        let identifier = self.app.config().identifier.as_str();
        let mut folders: Vec<PathBuf> = Vec::new();
        for folder in [path.app_data_dir(), path.app_cache_dir()]
            .into_iter()
            .flatten()
        {
            if folder.file_name().is_some_and(|name| name == identifier)
                && !folders.contains(&folder)
            {
                folders.push(folder);
            }
        }
        if let Some(shared) = maxhq_core::paths::data_dir()
            && shared
                .file_name()
                .is_some_and(|name| name == "MaxHQ" || name == "maxhq")
            && !folders.contains(&shared)
        {
            folders.push(shared);
        }
        folders
    }

    /// Clears what each webview keeps, such as an unsent draft, then
    /// quits a moment later.
    fn quit(&self) {
        for webview in self.app.webview_windows().into_values() {
            if let Err(error) = webview.clear_all_browsing_data() {
                tracing::warn!(%error, "couldn't clear what the webview keeps");
            }
        }
        let app = self.app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(QUIT_AFTER).await;
            app.exit(0);
        });
    }
}

/// Opens `url` in the app its scheme belongs to, which is the default
/// browser for a web page. `what` names it if it fails.
async fn open(url: String, what: &str) -> Result<(), String> {
    let opener = if cfg!(target_os = "macos") {
        "/usr/bin/open"
    } else {
        "xdg-open"
    };
    let status = tokio::process::Command::new(opener)
        .arg(&url)
        .status()
        .await
        .map_err(|error| format!("{what} didn't open: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{what} didn't open: {opener} ended with {status}"))
    }
}

/// Starts an editor's command line. It hands the open to the editor and
/// ends, and the runtime reaps it, so nothing waits on it here.
async fn spawn_editor(program: PathBuf, args: Vec<String>) -> Result<(), String> {
    tokio::process::Command::new(&program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
        .map_err(|error| format!("The editor didn't start: {error}"))
}
