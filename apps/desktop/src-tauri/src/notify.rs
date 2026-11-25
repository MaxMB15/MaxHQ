// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The notification sender for this platform, with your settings in front
//! of it, and the commands the settings page calls. The rules for when an
//! alert shows live in [`maxhq_app::notify`].

#[cfg(any(target_os = "linux", windows, test))]
mod content;
#[cfg(any(target_os = "linux", windows))]
mod desktop;
#[cfg(target_os = "macos")]
mod macos;

use std::sync::Arc;

use maxhq_app::notify::{
    test_notification, Filtered, LivePrefs, NotificationPrefs, NotificationSender,
};
use maxhq_app::App;
use maxhq_protocol::ids::AgentId;
use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::native::NativeError;

/// Whether this app's notifications show. On macOS that's up to System
/// Settings. On Linux they show when the desktop has a notification
/// server, and on Windows they always do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum NotificationPermission {
    /// macOS hasn't asked you yet.
    NotDecided,
    /// You turned them off in System Settings.
    Denied,
    /// They show.
    Allowed,
    /// This build can't show any: on macOS because it doesn't run from an
    /// app bundle, on Linux because no notification server answered.
    Unavailable,
}

/// What the system lets this app do, as it has it now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
pub struct NotificationAccess {
    /// Whether it shows notifications.
    pub permission: NotificationPermission,
    /// Whether it may badge its icon in the Dock. macOS hides the badge
    /// of an app that uses Notification Center without this, even when
    /// the app sets one.
    pub badge: bool,
}

impl NotificationAccess {
    /// Access where Notification Center isn't used, so nothing there
    /// hides the badge.
    const fn without_center(permission: NotificationPermission) -> Self {
        Self {
            permission,
            badge: true,
        }
    }
}

/// The system's sender and your settings for it, which the commands reach
/// through the app's state.
pub struct Notifications {
    system: Arc<dyn NotificationSender + Sync>,
    prefs: LivePrefs,
    /// What the system's sender is, for the commands that ask it about
    /// permission.
    kind: Kind,
}

/// Which sender shows this app's notifications.
enum Kind {
    /// Notification Center, on a Mac when the app runs from its bundle.
    #[cfg(target_os = "macos")]
    Center,
    /// The desktop's notifications on Linux, toasts on Windows.
    #[cfg(any(target_os = "linux", windows))]
    Desktop(Arc<desktop::Desktop>),
    /// Only the log.
    #[cfg(not(any(target_os = "linux", windows)))]
    Log,
}

impl Notifications {
    /// Starts the system's sender with `prefs`. The second half is the
    /// sender for the notifier, which shows only what the settings let
    /// through.
    pub fn start(
        on_click: impl Fn(AgentId) + Send + Sync + 'static,
        prefs: NotificationPrefs,
    ) -> (Self, Box<dyn NotificationSender>) {
        let (system, kind) = system_sender(on_click);
        let prefs = LivePrefs::new(prefs);
        let filtered = Filtered::new(system.clone(), prefs.clone());
        let this = Self {
            system,
            prefs,
            kind,
        };
        (this, Box::new(filtered))
    }
}

/// The sender for Linux and Windows, which shows notifications on the
/// desktop.
#[cfg(any(target_os = "linux", windows))]
fn system_sender(
    on_click: impl Fn(AgentId) + Send + Sync + 'static,
) -> (Arc<dyn NotificationSender + Sync>, Kind) {
    let desktop = Arc::new(desktop::Desktop::start(Box::new(on_click)));
    (desktop.clone(), Kind::Desktop(desktop))
}

/// The sender for this platform. On macOS it's Notification Center when the
/// app runs from its bundle. Anything else only logs.
#[cfg(not(any(target_os = "linux", windows)))]
fn system_sender(
    on_click: impl Fn(AgentId) + Send + Sync + 'static,
) -> (Arc<dyn NotificationSender + Sync>, Kind) {
    #[cfg(target_os = "macos")]
    if let Some(sender) = macos::Center::start(Box::new(on_click)) {
        return (Arc::new(sender), Kind::Center);
    }
    #[cfg(not(target_os = "macos"))]
    drop(on_click);
    tracing::info!("notifications only go to the log in this build");
    (Arc::new(maxhq_app::notify::LogSender), Kind::Log)
}

/// The notification settings now.
#[tauri::command]
#[specta::specta]
pub fn notification_prefs(notifications: State<'_, Notifications>) -> NotificationPrefs {
    notifications.prefs.get()
}

/// Changes the notification settings. They apply to the next alert, and
/// last across restarts.
#[tauri::command]
#[specta::specta]
pub fn set_notification_prefs(
    app: State<'_, App>,
    notifications: State<'_, Notifications>,
    prefs: NotificationPrefs,
) -> Result<(), NativeError> {
    notifications.prefs.set(prefs.clone());
    prefs
        .save(&app.devices().registry())
        .map_err(NativeError::failed)
}

/// Whether this app's notifications show and it may badge its icon: on
/// macOS as System Settings has it now, on Linux as the desktop answered
/// when the app started.
#[tauri::command]
#[specta::specta]
pub async fn notification_permission(
    notifications: State<'_, Notifications>,
) -> Result<NotificationAccess, NativeError> {
    match &notifications.kind {
        #[cfg(target_os = "macos")]
        Kind::Center => Ok(macos::access().await),
        #[cfg(any(target_os = "linux", windows))]
        Kind::Desktop(desktop) => Ok(NotificationAccess::without_center(
            desktop.permission().await,
        )),
        #[cfg(not(any(target_os = "linux", windows)))]
        Kind::Log => Ok(NotificationAccess::without_center(
            NotificationPermission::Unavailable,
        )),
    }
}

/// Shows a notification to try the settings out, whichever kinds are on.
/// macOS asks for permission first if it hasn't yet.
#[tauri::command]
#[specta::specta]
pub async fn send_test_notification(
    notifications: State<'_, Notifications>,
) -> Result<(), NativeError> {
    #[cfg(target_os = "macos")]
    if matches!(notifications.kind, Kind::Center) {
        macos::ask().await;
    }
    notifications
        .system
        .deliver(&test_notification(&notifications.prefs.get()));
    Ok(())
}

/// Opens this app's page under Notifications in System Settings. Linux and
/// Windows have no settings page to link to.
#[tauri::command]
#[specta::specta]
pub fn open_notification_settings() -> Result<(), NativeError> {
    #[cfg(target_os = "macos")]
    return macos::open_settings();
    #[cfg(not(target_os = "macos"))]
    Err(NativeError::Unsupported {
        what: "System Settings".into(),
    })
}
