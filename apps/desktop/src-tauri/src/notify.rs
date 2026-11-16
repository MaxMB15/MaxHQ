// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The notification sender for this platform, with your settings in front
//! of it, and the commands the settings page calls. The rules for when an
//! alert shows live in [`maxhq_app::notify`].

#[cfg(target_os = "macos")]
mod macos;

use std::sync::Arc;

use maxhq_app::notify::{
    test_notification, Filtered, LivePrefs, LogSender, NotificationPrefs, NotificationSender,
};
use maxhq_app::App;
use maxhq_protocol::ids::AgentId;
use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::native::NativeError;

/// Whether macOS lets this app show notifications. Off a Mac it's always
/// `Unavailable`.
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
    /// This build can't show any, because it doesn't run from an app
    /// bundle.
    Unavailable,
}

/// What macOS lets this app do, as System Settings has it now.
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
    /// A build that doesn't use Notification Center, so nothing there
    /// hides its badge.
    const UNAVAILABLE: Self = Self {
        permission: NotificationPermission::Unavailable,
        badge: true,
    };
}

/// The system's sender and your settings for it, which the commands reach
/// through the app's state.
pub struct Notifications {
    system: Arc<dyn NotificationSender + Sync>,
    prefs: LivePrefs,
    /// Whether the system's sender is Notification Center, rather than the
    /// log.
    center: bool,
}

impl Notifications {
    /// Starts the system's sender with `prefs`. The second half is the
    /// sender for the notifier, which shows only what the settings let
    /// through.
    pub fn start(
        on_click: impl Fn(AgentId) + Send + Sync + 'static,
        prefs: NotificationPrefs,
    ) -> (Self, Box<dyn NotificationSender>) {
        let (system, center) = system_sender(on_click);
        let prefs = LivePrefs::new(prefs);
        let filtered = Filtered::new(system.clone(), prefs.clone());
        let this = Self {
            system,
            prefs,
            center,
        };
        (this, Box::new(filtered))
    }
}

/// The sender for this platform, and whether it's Notification Center. On
/// macOS it is when the app runs from its bundle. Anything else only logs.
fn system_sender(
    on_click: impl Fn(AgentId) + Send + Sync + 'static,
) -> (Arc<dyn NotificationSender + Sync>, bool) {
    #[cfg(target_os = "macos")]
    if let Some(sender) = macos::Center::start(Box::new(on_click)) {
        return (Arc::new(sender), true);
    }
    #[cfg(not(target_os = "macos"))]
    drop(on_click);
    tracing::info!("notifications only go to the log in this build");
    (Arc::new(LogSender), false)
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

/// Whether macOS lets this app show notifications and badge its icon, as
/// System Settings has it now.
#[tauri::command]
#[specta::specta]
pub async fn notification_permission(
    notifications: State<'_, Notifications>,
) -> Result<NotificationAccess, NativeError> {
    if !notifications.center {
        return Ok(NotificationAccess::UNAVAILABLE);
    }
    #[cfg(target_os = "macos")]
    return Ok(macos::access().await);
    #[cfg(not(target_os = "macos"))]
    Ok(NotificationAccess::UNAVAILABLE)
}

/// Shows a notification to try the settings out, whichever kinds are on.
/// macOS asks for permission first if it hasn't yet.
#[tauri::command]
#[specta::specta]
pub async fn send_test_notification(
    notifications: State<'_, Notifications>,
) -> Result<(), NativeError> {
    #[cfg(target_os = "macos")]
    if notifications.center {
        macos::ask().await;
    }
    notifications
        .system
        .deliver(&test_notification(&notifications.prefs.get()));
    Ok(())
}

/// Opens this app's page under Notifications in System Settings.
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
