// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The app's own update, through Tauri's updater: it looks for a newer
//! release on GitHub a little after the app starts and every few hours
//! while your setting lets it, and on Check now. When you choose Update now
//! it downloads the release, checks its signature, puts it in place and
//! restarts the app. The webview only calls the commands here, so it needs
//! no updater permission of its own. The rules for when the app offers
//! updates are in [`maxhq_app::app_update`].

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use maxhq_app::app_update::{
    self, NoUpdates, UpdatePrefs, UpdateStatus, CHECK_EVERY, FIRST_CHECK_AFTER,
};
use maxhq_app::App;
use maxhq_protocol::time::Timestamp;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};
use tauri_specta::Event;

use crate::native::NativeError;

/// How long a look for an update may take.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// Sent whenever the update's status changes, so the prompt and the
/// settings page follow it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, Event)]
pub struct AppUpdateChanged(pub UpdateStatus);

/// What the settings page shows about the app's own update.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Type)]
pub struct AppUpdate {
    /// This app's version.
    pub version: String,
    /// Where its update stands.
    pub status: UpdateStatus,
    /// Your settings for it.
    pub prefs: UpdatePrefs,
    /// The page that lists the releases, taken from where the updater
    /// looks, when that is a GitHub release download. A release's notes are
    /// at `<release_notes>/tag/v<version>`.
    pub release_notes: Option<String>,
}

/// The update's state, which the commands reach through the app's state.
pub struct Updates {
    offered: Result<(), NoUpdates>,
    inner: Mutex<Inner>,
    /// Held while the app looks or installs, so only one runs at a time.
    busy: tokio::sync::Mutex<()>,
}

struct Inner {
    status: UpdateStatus,
    found: Option<Update>,
    prefs: UpdatePrefs,
}

impl Updates {
    /// The update state for this copy of the app, with your saved `prefs`.
    pub fn new(prefs: UpdatePrefs) -> Self {
        let offered = app_update::Install::this_app()
            .ok_or(NoUpdates::FromSource)
            .and_then(|app| app.offers_updates(cfg!(debug_assertions)));
        if let Err(reason) = offered {
            tracing::info!(?reason, "this copy of the app doesn't offer updates");
        }
        let status = match offered {
            Ok(()) => UpdateStatus::Idle,
            Err(reason) => UpdateStatus::Unavailable { reason },
        };
        Self {
            offered,
            inner: Mutex::new(Inner {
                status,
                found: None,
                prefs,
            }),
            busy: tokio::sync::Mutex::new(()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn status(&self) -> UpdateStatus {
        self.lock().status.clone()
    }

    /// Sets the status and tells the webview.
    fn set(&self, app: &AppHandle, status: UpdateStatus) {
        self.lock().status = status.clone();
        tell(app, &status);
    }

    /// Looks for a newer release, and says what it found. While the app
    /// already looks or installs, it says where that stands instead.
    pub async fn check(&self, app: &AppHandle) -> UpdateStatus {
        if let Err(reason) = self.offered {
            return UpdateStatus::Unavailable { reason };
        }
        let Ok(_busy) = self.busy.try_lock() else {
            return self.status();
        };
        self.set(app, UpdateStatus::Checking);
        let checked = match app.updater_builder().timeout(CHECK_TIMEOUT).build() {
            Ok(updater) => updater.check().await,
            Err(error) => Err(error),
        };
        let status = match checked {
            Ok(Some(update)) => {
                let version = update.version.clone();
                tracing::info!(%version, "a newer version of the app is out");
                self.lock().found = Some(update);
                UpdateStatus::Available { version }
            }
            Ok(None) => {
                self.lock().found = None;
                UpdateStatus::UpToDate {
                    checked_at: Timestamp::now(),
                }
            }
            Err(error) => {
                tracing::warn!(%error, "couldn't look for an update");
                // An update found before stays on offer.
                let found = self
                    .lock()
                    .found
                    .as_ref()
                    .map(|found| found.version.clone());
                match found {
                    Some(version) => UpdateStatus::Available { version },
                    None => UpdateStatus::Failed {
                        reason: error.to_string(),
                        version: None,
                    },
                }
            }
        };
        self.set(app, status.clone());
        status
    }

    /// Downloads the update found last, checks its signature, puts it in
    /// place and restarts the app. Returns only when that fails.
    async fn install(&self, app: &AppHandle) -> Result<(), NativeError> {
        if let Err(reason) = self.offered {
            return Err(NativeError::failed(format!(
                "This copy of the app doesn't update itself ({reason:?})"
            )));
        }
        let Ok(_busy) = self.busy.try_lock() else {
            return Err(NativeError::failed("An update is already under way"));
        };
        let Some(update) = self.lock().found.clone() else {
            return Err(NativeError::failed(
                "There's no update to install. Check for one first",
            ));
        };
        let version = update.version.clone();
        self.set(
            app,
            UpdateStatus::Downloading {
                version: version.clone(),
                downloaded: 0,
                total: None,
            },
        );
        let mut downloaded = 0u64;
        let mut shown = Progress::default();
        let installed = update
            .download_and_install(
                |chunk, total| {
                    downloaded += chunk as u64;
                    if shown.moved(downloaded, total) {
                        self.set(
                            app,
                            UpdateStatus::Downloading {
                                version: version.clone(),
                                downloaded,
                                total,
                            },
                        );
                    }
                },
                || {
                    self.set(
                        app,
                        UpdateStatus::Installing {
                            version: version.clone(),
                        },
                    )
                },
            )
            .await;
        match installed {
            Ok(()) => {
                tracing::info!(%version, "installed the update, restarting");
                app.restart()
            }
            Err(error) => {
                tracing::warn!(%error, %version, "couldn't install the update");
                let reason = error.to_string();
                self.set(
                    app,
                    UpdateStatus::Failed {
                        reason: reason.clone(),
                        version: Some(version),
                    },
                );
                Err(NativeError::Failed { message: reason })
            }
        }
    }
}

/// How far the download has got, as last shown. It moves on each whole
/// percent, or each megabyte when the size isn't known, so the webview
/// isn't sent every chunk.
#[derive(Default)]
struct Progress {
    last: Option<u64>,
}

impl Progress {
    fn moved(&mut self, downloaded: u64, total: Option<u64>) -> bool {
        let step = match total {
            Some(total) if total > 0 => downloaded.saturating_mul(100) / total,
            _ => downloaded / (1 << 20),
        };
        if self.last == Some(step) {
            return false;
        }
        self.last = Some(step);
        true
    }
}

/// Tells the webview the status changed.
fn tell(app: &AppHandle, status: &UpdateStatus) {
    if let Err(error) = AppUpdateChanged(status.clone()).emit(app) {
        tracing::warn!(%error, "couldn't tell the webview about the update");
    }
}

/// Looks for an update a little after the app starts and every few hours
/// after, while your setting lets it.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_AFTER).await;
        loop {
            let updates = app.state::<Updates>();
            if updates.offered.is_ok() && updates.lock().prefs.auto_check {
                updates.check(&app).await;
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// This app's version, where its update stands and your settings for it.
#[tauri::command]
#[specta::specta]
pub fn app_update(app: AppHandle, updates: State<'_, Updates>) -> AppUpdate {
    let inner = updates.lock();
    AppUpdate {
        version: app.package_info().version.to_string(),
        status: inner.status.clone(),
        prefs: inner.prefs,
        release_notes: release_notes(&app),
    }
}

/// Where the updater's first endpoint says the releases are.
fn release_notes(app: &AppHandle) -> Option<String> {
    let endpoint = app
        .config()
        .plugins
        .0
        .get("updater")?
        .get("endpoints")?
        .get(0)?
        .as_str()?;
    releases_page(endpoint)
}

/// The releases page behind a `.../releases/latest/download/<file>` link.
fn releases_page(endpoint: &str) -> Option<String> {
    let (base, _file) = endpoint.split_once("/releases/latest/download/")?;
    base.starts_with("https://")
        .then(|| format!("{base}/releases"))
}

/// Looks for a newer version now, whatever the setting says.
#[tauri::command]
#[specta::specta]
pub async fn check_for_update(
    app: AppHandle,
    updates: State<'_, Updates>,
) -> Result<UpdateStatus, NativeError> {
    Ok(updates.check(&app).await)
}

/// Downloads and installs the newer version found, then restarts the app.
/// It answers only when that fails.
#[tauri::command]
#[specta::specta]
pub async fn install_update(
    app: AppHandle,
    updates: State<'_, Updates>,
) -> Result<(), NativeError> {
    updates.install(&app).await
}

/// Changes the update settings. They last across restarts.
#[tauri::command]
#[specta::specta]
pub fn set_update_prefs(
    app: State<'_, App>,
    updates: State<'_, Updates>,
    prefs: UpdatePrefs,
) -> Result<(), NativeError> {
    updates.lock().prefs = prefs;
    prefs
        .save(&app.devices().registry())
        .map_err(NativeError::failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_page_comes_from_a_release_download() {
        assert_eq!(
            releases_page("https://github.com/OWNER/REPO/releases/latest/download/latest.json")
                .as_deref(),
            Some("https://github.com/OWNER/REPO/releases")
        );
        assert_eq!(releases_page("https://example.com/latest.json"), None);
        assert_eq!(
            releases_page("http://example.com/releases/latest/download/latest.json"),
            None
        );
    }

    #[test]
    fn an_update_installs_only_as_the_version_its_signature_names() {
        // The release signs each archive with its version, so a manifest
        // that names a newer version can't hand out an older release.
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(
            config["plugins"]["updater"]["requireSignedVersion"],
            serde_json::Value::Bool(true)
        );
    }

    #[test]
    fn progress_moves_on_each_percent_or_megabyte() {
        let mut shown = Progress::default();
        assert!(shown.moved(0, Some(1000)));
        assert!(!shown.moved(5, Some(1000)));
        assert!(shown.moved(10, Some(1000)));
        assert!(!shown.moved(19, Some(1000)));
        assert!(shown.moved(1000, Some(1000)));

        let mut unknown = Progress::default();
        assert!(unknown.moved(10, None));
        assert!(!unknown.moved(1 << 19, None));
        assert!(unknown.moved(1 << 20, None));
    }
}
