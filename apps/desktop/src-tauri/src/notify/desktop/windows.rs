// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Toasts, through the toast notifier for the app's AppUserModelID. The
//! app claims that ID as it starts, so Windows hands a click on one of its
//! toasts back to this process, which opens the agent. Windows reports the
//! click only to a toast the app still holds, so the app keeps the newest
//! ones. A click once the app has quit opens it, without the agent.
//! Taking a toast off screen goes by its tag and the app's group in the
//! ID's history, which also clears what Action Center keeps.

use std::collections::VecDeque;
use std::sync::Arc;

use maxhq_app::notify::Notification;
use maxhq_protocol::ids::AgentId;
use windows::core::{IInspectable, Ref, Result, HSTRING};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{
    NotificationSetting, ToastNotification, ToastNotificationManager, ToastNotifier,
};

use super::super::content::{toast_tag, toast_xml, TOAST_GROUP};
use super::super::NotificationPermission;
use super::{Abilities, Content, OnClick};
use crate::native::windows::APP_ID;

/// How many toasts the app holds on to for their clicks. An agent shows
/// one at a time, each replacing the one before, so this covers far more
/// agents than wait at once.
const HELD: usize = 64;

/// Windows reports a toast's clicks on threads of its own, so the thread
/// waits on its queue alone.
pub use std::sync::mpsc::{channel as jobs, Receiver as Queue, Sender as Jobs};

/// The toasts the app has shown.
pub struct Screen {
    on_click: Arc<dyn Fn(AgentId) + Send + Sync>,
    /// The app's notifier, when Windows gave one.
    notifier: Option<ToastNotifier>,
    /// The toasts on screen or in Action Center, newest last, by tag.
    held: VecDeque<(String, ToastNotification)>,
}

impl Screen {
    pub fn new(on_click: OnClick) -> Self {
        let notifier = notifier()
            .inspect_err(|error| tracing::warn!(%error, "Windows has no toasts for the app"))
            .ok();
        Self {
            on_click: Arc::from(on_click),
            notifier,
            held: VecDeque::new(),
        }
    }

    /// The next job, or none once the app lets go of the queue.
    pub fn next<T>(&mut self, queue: &Queue<T>) -> Option<T> {
        queue.recv().ok()
    }

    /// Toasts show when Windows gave a notifier, with a line for the
    /// subtitle, and report clicks.
    pub fn abilities(&mut self) -> Abilities {
        Abilities {
            shows: self.notifier.is_some(),
            clicks: true,
            markup: false,
            subtitle: true,
        }
    }

    pub fn show(&mut self, notification: &Notification, content: &Content) {
        if let Err(error) = self.try_show(notification, content) {
            tracing::warn!(%error, id = notification.id, "Windows didn't show a toast");
        }
    }

    fn try_show(&mut self, notification: &Notification, content: &Content) -> Result<()> {
        let Some(notifier) = &self.notifier else {
            return Ok(());
        };
        let xml = XmlDocument::new()?;
        xml.LoadXml(&HSTRING::from(toast_xml(content)))?;
        let toast = ToastNotification::CreateToastNotification(&xml)?;
        let tag = toast_tag(&notification.id);
        toast.SetTag(&HSTRING::from(&tag))?;
        toast.SetGroup(&HSTRING::from(TOAST_GROUP))?;
        let (on_click, agent) = (self.on_click.clone(), notification.agent);
        toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
            move |_: Ref<ToastNotification>, _: Ref<IInspectable>| {
                on_click(agent);
                Ok(())
            },
        ))?;
        notifier.Show(&toast)?;
        self.hold(tag, toast);
        Ok(())
    }

    /// Holds `toast` in place of any with its tag, which showing it
    /// replaced, and lets the oldest go past [`HELD`].
    fn hold(&mut self, tag: String, toast: ToastNotification) {
        self.held.retain(|(each, _)| *each != tag);
        if self.held.len() >= HELD {
            self.held.pop_front();
        }
        self.held.push_back((tag, toast));
    }

    pub fn remove(&mut self, ids: &[String]) {
        let history = match ToastNotificationManager::History() {
            Ok(history) => Some(history),
            Err(error) => {
                tracing::warn!(%error, "Windows gave no toast history to take toasts from");
                None
            }
        };
        let (group, app) = (HSTRING::from(TOAST_GROUP), HSTRING::from(APP_ID));
        for id in ids {
            let tag = toast_tag(id);
            if let Some(history) = &history
                && let Err(error) =
                    history.RemoveGroupedTagWithId(&HSTRING::from(&tag), &group, &app)
            {
                // Windows errs for a toast that's gone already.
                tracing::debug!(%error, id, "Windows didn't take a toast off screen");
            }
            self.held.retain(|(each, _)| *each != tag);
        }
    }

    pub fn remove_all(&mut self) {
        let cleared = ToastNotificationManager::History()
            .and_then(|history| history.ClearWithId(&HSTRING::from(APP_ID)));
        if let Err(error) = cleared {
            tracing::warn!(%error, "Windows didn't take the app's toasts off screen");
        }
        self.held.clear();
    }
}

/// The notifier for the app's ID.
fn notifier() -> Result<ToastNotifier> {
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))
}

/// Whether Windows shows the app's toasts as its settings have it now,
/// which can change while the app runs.
pub fn permission() -> NotificationPermission {
    match notifier().and_then(|notifier| notifier.Setting()) {
        Ok(setting) => permission_of(setting),
        Err(error) => {
            tracing::warn!(%error, "Windows didn't say whether it shows the app's toasts");
            NotificationPermission::Unavailable
        }
    }
}

/// The permission the settings page shows for `setting`: off in Windows
/// Settings, or by an administrator's policy, reads as turned off, and
/// anything Windows can't show toasts for at all as unavailable.
fn permission_of(setting: NotificationSetting) -> NotificationPermission {
    if setting == NotificationSetting::Enabled {
        NotificationPermission::Allowed
    } else if setting == NotificationSetting::DisabledForApplication
        || setting == NotificationSetting::DisabledForUser
        || setting == NotificationSetting::DisabledByGroupPolicy
    {
        NotificationPermission::Denied
    } else {
        NotificationPermission::Unavailable
    }
}
