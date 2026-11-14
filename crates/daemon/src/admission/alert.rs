// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The macOS alert that shows an admission card on a Mac with no proven
//! desktop app connected, so a program can't wait for a moment nobody
//! looks at the app.
//!
//! It's a `CFUserNotification`, which a process with no window and no
//! entitlements can show, under the hardened runtime too. "Don't allow" is
//! its default button, so Return keeps the program out, and it closes as
//! Don't allow when the card expires. Off the Mac there's no alert, and in
//! tests a fake stands in.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use super::words::Words;

/// What came of an alert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum Reply {
    /// You picked the allow button.
    Allow,
    /// You picked Don't allow, or nobody answered before the card expired.
    DontAllow,
    /// The device took the alert down itself, so it says nothing about the
    /// card.
    Closed,
}

/// Shows an admission card outside the app.
pub trait Alert: Send + Sync {
    /// Shows `words` for at most `wait` and blocks until you answer, the
    /// wait passes or `cancel` takes it down.
    fn ask(&self, words: &Words, wait: Duration, cancel: &Cancel) -> Reply;
}

/// Takes an alert down from another thread.
#[derive(Clone, Default)]
pub struct Cancel(Arc<Mutex<Cancelling>>);

#[derive(Default)]
struct Cancelling {
    cancelled: bool,
    hook: Option<Box<dyn FnMut() + Send>>,
}

impl Cancel {
    /// Takes the alert down, now or as soon as it shows.
    pub fn cancel(&self) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.cancelled {
            return;
        }
        state.cancelled = true;
        if let Some(mut hook) = state.hook.take() {
            hook();
        }
    }

    /// Whether the alert was taken down.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_cancelled(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .cancelled
    }

    /// Runs `hook` once the alert is taken down. False, and `hook` doesn't
    /// run, when it already was.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn on_cancel(&self, hook: Box<dyn FnMut() + Send>) -> bool {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.cancelled {
            return false;
        }
        state.hook = Some(hook);
        true
    }

    /// Forgets the hook, once the alert it closes is gone.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn clear(&self) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).hook = None;
    }
}

/// The alert this platform shows: the macOS one on a Mac, none elsewhere
/// and none in tests, which never show a real one.
pub fn platform() -> Option<Arc<dyn Alert>> {
    #[cfg(all(target_os = "macos", not(test)))]
    {
        Some(Arc::new(mac::MacAlert))
    }
    #[cfg(not(all(target_os = "macos", not(test))))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
#[cfg_attr(test, allow(dead_code))]
mod mac {
    use std::ffi::c_void;
    use std::time::Duration;

    use core_foundation::base::{CFOptionFlags, CFRelease, CFType, TCFType};
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::{CFString, CFStringRef};
    use tracing::warn;

    use super::{Alert, Cancel, Reply, Words};
    use crate::admission::words::DONT_ALLOW;

    /// `kCFUserNotificationCautionAlertLevel`.
    const CAUTION: CFOptionFlags = 2;
    /// `kCFUserNotificationAlternateResponse`: the second button.
    const ALTERNATE: CFOptionFlags = 1;
    /// The bits of a response that say which button.
    const BUTTON: CFOptionFlags = 0x3;

    #[allow(unsafe_code)]
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFUserNotificationAlertHeaderKey: CFStringRef;
        static kCFUserNotificationAlertMessageKey: CFStringRef;
        static kCFUserNotificationDefaultButtonTitleKey: CFStringRef;
        static kCFUserNotificationAlternateButtonTitleKey: CFStringRef;
        fn CFUserNotificationCreate(
            allocator: *const c_void,
            timeout: f64,
            flags: CFOptionFlags,
            error: *mut i32,
            dictionary: CFDictionaryRef,
        ) -> *mut c_void;
        fn CFUserNotificationReceiveResponse(
            notification: *mut c_void,
            timeout: f64,
            response: *mut CFOptionFlags,
        ) -> i32;
        fn CFUserNotificationCancel(notification: *mut c_void) -> i32;
    }

    /// The alert, shown by the system's notification center.
    pub struct MacAlert;

    /// A notification on screen, which another thread may take down.
    struct Shown(*mut c_void);

    // SAFETY: a CFUserNotification may be cancelled from any thread, which
    // is all the other thread does with it, and it's released only after
    // the hook that holds it is gone.
    #[allow(unsafe_code)]
    unsafe impl Send for Shown {}

    impl Shown {
        #[allow(unsafe_code)]
        fn cancel(&self) {
            // SAFETY: the notification is live until the one that shows it
            // releases it, after forgetting this hook.
            unsafe {
                CFUserNotificationCancel(self.0);
            }
        }
    }

    #[allow(unsafe_code)]
    fn key(key: CFStringRef) -> CFString {
        // SAFETY: the keys are constants the framework keeps for the life
        // of the process, so taking them under the get rule retains them.
        unsafe { CFString::wrap_under_get_rule(key) }
    }

    impl Alert for MacAlert {
        #[allow(unsafe_code)]
        fn ask(&self, words: &Words, wait: Duration, cancel: &Cancel) -> Reply {
            // SAFETY: the keys are framework constants.
            let (header, message, first, second) = unsafe {
                (
                    key(kCFUserNotificationAlertHeaderKey),
                    key(kCFUserNotificationAlertMessageKey),
                    key(kCFUserNotificationDefaultButtonTitleKey),
                    key(kCFUserNotificationAlternateButtonTitleKey),
                )
            };
            let pairs: Vec<(CFString, CFType)> = vec![
                (header, CFString::new(&words.title).as_CFType()),
                (message, CFString::new(&words.body()).as_CFType()),
                (first, CFString::new(DONT_ALLOW).as_CFType()),
                (second, CFString::new(&words.allow).as_CFType()),
            ];
            let dictionary = CFDictionary::from_CFType_pairs(&pairs);
            let seconds = wait.as_secs_f64().max(1.0);
            let mut error = 0;
            // SAFETY: the dictionary is live for the call, which copies
            // what it needs, and the result is ours to release.
            let note = unsafe {
                CFUserNotificationCreate(
                    std::ptr::null(),
                    seconds,
                    CAUTION,
                    &raw mut error,
                    dictionary.as_concrete_TypeRef(),
                )
            };
            if note.is_null() {
                warn!(error, "couldn't show the admission alert");
                return Reply::Closed;
            }
            let shown = Shown(note);
            if !cancel.on_cancel(Box::new(move || shown.cancel())) {
                // SAFETY: the notification is live and ours.
                unsafe {
                    CFUserNotificationCancel(note);
                    CFRelease(note.cast_const());
                }
                return Reply::Closed;
            }
            let mut response: CFOptionFlags = 0;
            // SAFETY: the notification is live until the release below,
            // and the call writes one flags value. A timed-out alert closes
            // itself, so a little past its own timeout always returns.
            let status = unsafe {
                CFUserNotificationReceiveResponse(note, seconds + 1.0, &raw mut response)
            };
            cancel.clear();
            // SAFETY: the hook that held the notification is gone, so this
            // is the last use.
            unsafe { CFRelease(note.cast_const()) };
            if cancel.is_cancelled() {
                Reply::Closed
            } else if status == 0 && response & BUTTON == ALTERNATE {
                Reply::Allow
            } else {
                Reply::DontAllow
            }
        }
    }
}

/// An alert that shows nothing, for tests.
#[cfg(test)]
pub mod testing {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::{Duration, Instant};

    use super::{Alert, Cancel, Reply};
    use crate::admission::words::Words;

    /// Records what it shows, and answers with what a test queued.
    #[derive(Default)]
    pub struct Fake(Mutex<Seen>);

    #[derive(Default)]
    struct Seen {
        shown: Vec<Words>,
        replies: VecDeque<Reply>,
        closed: usize,
    }

    impl Fake {
        /// A fake with nothing shown.
        pub fn new() -> Arc<Self> {
            Arc::default()
        }

        /// Answers the alert up now, or the next one, with `reply`.
        pub fn reply(&self, reply: Reply) {
            self.lock().replies.push_back(reply);
        }

        /// Everything it showed, oldest first.
        pub fn shown(&self) -> Vec<Words> {
            self.lock().shown.clone()
        }

        /// How many alerts the device took down itself.
        pub fn closed(&self) -> usize {
            self.lock().closed
        }

        fn lock(&self) -> std::sync::MutexGuard<'_, Seen> {
            self.0.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    impl Alert for Fake {
        fn ask(&self, words: &Words, wait: Duration, cancel: &Cancel) -> Reply {
            self.lock().shown.push(words.clone());
            let deadline = Instant::now() + wait;
            loop {
                if cancel.is_cancelled() {
                    self.lock().closed += 1;
                    return Reply::Closed;
                }
                if let Some(reply) = self.lock().replies.pop_front() {
                    return reply;
                }
                if Instant::now() >= deadline {
                    return Reply::DontAllow;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}
