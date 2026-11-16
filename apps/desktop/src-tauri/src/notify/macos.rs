// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Notifications through `UNUserNotificationCenter`.
//!
//! The center only works for a process launched from an app bundle. A bare
//! binary, such as the one `tauri dev` runs, raises an Objective-C
//! exception on first use, so [`Center::start`] checks for a bundle first.
//! The agent each notification is about rides in its thread identifier,
//! which also groups an agent's notifications together, and a click hands
//! it back.
//!
//! macOS hides the Dock badge of an app that uses the center unless the
//! app asked for badges and you leave them on, so the app asks for them
//! along with alerts and sounds.
//!
//! `pnpm dev` runs the debug build from a small bundle of its own, with
//! `.dev` on the end of the app's identifier, so macOS asks about it and
//! lists it in System Settings apart from the app you installed.

// Declaring the delegate class needs `unsafe` for the superclass and the
// protocol. Every call into the center itself is safe.
#![allow(unsafe_code)]

use std::ptr::NonNull;
use std::sync::Mutex;

use block2::RcBlock;
use maxhq_app::notify::{Notification, NotificationSender};
use maxhq_protocol::ids::AgentId;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSArray, NSBundle, NSError, NSString, NSURL};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSetting, UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};
use tokio::sync::oneshot;

use super::{NotificationAccess, NotificationPermission};
use crate::native::NativeError;

/// What a click calls with the agent it was about.
type OnClick = Box<dyn Fn(AgentId) + Send + Sync>;

/// The delegate's state.
struct Ivars {
    on_click: OnClick,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the class has
    // no `Drop` impl.
    #[unsafe(super(NSObject))]
    #[name = "MaxHQNotificationDelegate"]
    #[ivars = Ivars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: Each method's signature matches the protocol's.
    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        /// Shows notifications while the app is frontmost too. The notifier
        /// already kept quiet about the pane you're looking at.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion_handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion_handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        /// A click on a notification. The app registers no actions, so
        /// every response is the default one, which is a click.
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion_handler: &block2::DynBlock<dyn Fn()>,
        ) {
            let thread = response
                .notification()
                .request()
                .content()
                .threadIdentifier()
                .to_string();
            match thread.parse::<AgentId>() {
                Ok(agent) => (self.ivars().on_click)(agent),
                Err(_) => tracing::warn!(thread, "a notification without an agent was clicked"),
            }
            completion_handler.call(());
        }
    }
);

impl Delegate {
    /// A delegate that hands each click to `on_click`.
    fn new(on_click: OnClick) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { on_click });
        // SAFETY: NSObject's init takes no arguments and returns the object.
        unsafe { msg_send![super(this), init] }
    }
}

/// Posts to Notification Center.
pub struct Center;

impl Center {
    /// Sets up Notification Center for this app and asks once for
    /// permission to alert and badge. Returns none when the app isn't
    /// running from its bundle, where the center can't work.
    pub fn start(on_click: OnClick) -> Option<Self> {
        let bundle = NSBundle::mainBundle();
        let path = bundle.bundlePath().to_string();
        if !path.ends_with(".app") || bundle.bundleIdentifier().is_none() {
            tracing::info!(path, "not running from an app bundle");
            return None;
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let delegate = Delegate::new(on_click);
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        // The center holds its delegate weakly, and this one has to last as
        // long as the app does.
        std::mem::forget(delegate);
        request(|granted| {
            if granted {
                tracing::info!("notifications are allowed");
            } else {
                tracing::warn!("notifications are turned off for MaxHQ in System Settings");
            }
        });
        Some(Self)
    }
}

/// Asks for permission to alert, play sounds and badge the icon, and calls
/// `answered` with whether it's given. macOS asks only the first time,
/// then answers from your settings. An app that asked before without
/// badges gets no new prompt for them.
fn request(answered: impl Fn(bool) + 'static) {
    let answered = RcBlock::new(move |granted: Bool, _error: *mut NSError| {
        answered(granted.as_bool());
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert
                | UNAuthorizationOptions::Sound
                | UNAuthorizationOptions::Badge,
            &answered,
        );
}

/// Asks for permission to alert and badge, if macOS hasn't yet, and waits
/// for the answer.
pub async fn ask() {
    let _ = asked().await;
}

/// Asks for permission to alert and badge. The answer comes through the
/// receiver.
fn asked() -> oneshot::Receiver<bool> {
    let (tell, told) = oneshot::channel();
    let tell = Once::new(tell);
    request(move |granted| tell.send(granted));
    told
}

/// Whether macOS lets this app show notifications and badge its icon now.
pub async fn access() -> NotificationAccess {
    settings().await.unwrap_or_else(|_| {
        // The center dropped the block without calling it.
        access_from(
            UNAuthorizationStatus::NotDetermined,
            UNNotificationSetting::NotSupported,
        )
    })
}

/// What the app's authorization and badge setting let it do.
fn access_from(status: UNAuthorizationStatus, badge: UNNotificationSetting) -> NotificationAccess {
    let permission = match status {
        UNAuthorizationStatus::NotDetermined => NotificationPermission::NotDecided,
        UNAuthorizationStatus::Denied => NotificationPermission::Denied,
        _ => NotificationPermission::Allowed,
    };
    NotificationAccess {
        permission,
        badge: badge == UNNotificationSetting::Enabled,
    }
}

/// Reads the app's settings from the center. The answer comes through the
/// receiver. The block stays here, since it can't cross threads.
fn settings() -> oneshot::Receiver<NotificationAccess> {
    let (tell, told) = oneshot::channel();
    let tell = Once::new(tell);
    let read = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        // SAFETY: The center hands the block settings that live at least
        // as long as the call.
        let settings = unsafe { settings.as_ref() };
        tell.send(access_from(
            settings.authorizationStatus(),
            settings.badgeSetting(),
        ));
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .getNotificationSettingsWithCompletionHandler(&read);
    told
}

/// A sender a block can call more than once, though only the first call
/// sends.
struct Once<T>(Mutex<Option<oneshot::Sender<T>>>);

impl<T> Once<T> {
    fn new(tell: oneshot::Sender<T>) -> Self {
        Self(Mutex::new(Some(tell)))
    }

    fn send(&self, value: T) {
        let tell = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(tell) = tell {
            let _ = tell.send(value);
        }
    }
}

/// Opens this app's page under Notifications in System Settings.
pub fn open_settings() -> Result<(), NativeError> {
    let id = NSBundle::mainBundle()
        .bundleIdentifier()
        .map(|id| id.to_string())
        .unwrap_or_default();
    let link =
        format!("x-apple.systempreferences:com.apple.Notifications-Settings.extension?id={id}");
    let url = NSURL::URLWithString(&NSString::from_str(&link))
        .ok_or_else(|| NativeError::failed("that isn't a System Settings link"))?;
    if NSWorkspace::sharedWorkspace().openURL(&url) {
        Ok(())
    } else {
        Err(NativeError::failed("System Settings didn't open"))
    }
}

impl NotificationSender for Center {
    fn deliver(&self, notification: &Notification) {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&notification.title));
        if let Some(subtitle) = &notification.subtitle {
            content.setSubtitle(&NSString::from_str(subtitle));
        }
        content.setBody(&NSString::from_str(&notification.body));
        content.setThreadIdentifier(&NSString::from_str(&notification.agent.to_string()));
        if notification.sound {
            content.setSound(Some(&UNNotificationSound::defaultSound()));
        }
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&notification.id),
            &content,
            None,
        );
        let id = notification.id.clone();
        let posted = RcBlock::new(move |error: *mut NSError| {
            if !error.is_null() {
                tracing::warn!(id, "Notification Center refused a notification");
            }
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .addNotificationRequest_withCompletionHandler(&request, Some(&posted));
    }

    fn remove(&self, ids: &[String]) {
        let ids: Vec<Retained<NSString>> = ids.iter().map(|id| NSString::from_str(id)).collect();
        let ids = NSArray::from_retained_slice(&ids);
        let center = UNUserNotificationCenter::currentNotificationCenter();
        center.removeDeliveredNotificationsWithIdentifiers(&ids);
        center.removePendingNotificationRequestsWithIdentifiers(&ids);
    }

    fn remove_all(&self) {
        let center = UNUserNotificationCenter::currentNotificationCenter();
        center.removeAllDeliveredNotifications();
        center.removeAllPendingNotificationRequests();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_status_gives_its_permission() {
        use NotificationPermission::{Allowed, Denied, NotDecided};
        for (status, permission) in [
            (UNAuthorizationStatus::NotDetermined, NotDecided),
            (UNAuthorizationStatus::Denied, Denied),
            (UNAuthorizationStatus::Authorized, Allowed),
            (UNAuthorizationStatus::Provisional, Allowed),
            (UNAuthorizationStatus::Ephemeral, Allowed),
        ] {
            let access = access_from(status, UNNotificationSetting::Enabled);
            assert_eq!(access.permission, permission, "{status:?}");
        }
    }

    #[test]
    fn only_a_badge_setting_that_is_on_lets_the_badge_show() {
        for (setting, badge) in [
            (UNNotificationSetting::NotSupported, false),
            (UNNotificationSetting::Disabled, false),
            (UNNotificationSetting::Enabled, true),
        ] {
            let access = access_from(UNAuthorizationStatus::Authorized, setting);
            assert_eq!(access.badge, badge, "{setting:?}");
        }
    }
}
