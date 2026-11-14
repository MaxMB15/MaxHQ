// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Keeping the Mac awake, per decision A-11. A sleeping Mac stops `maxd`, so
//! the device's settings say when it holds an IOKit assertion that stops idle
//! sleep. The display still turns off, and closing the lid still sleeps the
//! Mac. Other systems don't sleep under an agent, so there it does nothing.

use std::sync::Arc;

use maxhq_protocol::model::KeepAwake;

/// Holds the device awake when its mode says to.
pub trait Power: Send + Sync {
    /// Says which mode applies. It takes effect at once.
    fn set_mode(&self, mode: KeepAwake);

    /// Says whether any agent here is busy. It's called after every input,
    /// so it does nothing unless the answer changed.
    fn set_busy(&self, busy: bool);
}

/// Whether to hold the device awake in this mode.
#[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]
pub fn should_hold(mode: KeepAwake, busy: bool, on_charger: bool) -> bool {
    match mode {
        KeepAwake::Charger => on_charger || busy,
        KeepAwake::Always => true,
        KeepAwake::Working => busy,
        KeepAwake::Off => false,
    }
}

/// The power control for this system.
pub fn system() -> Arc<dyn Power> {
    #[cfg(target_os = "macos")]
    {
        mac::Awake::start()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Arc::new(NoPower)
    }
}

/// Power control that does nothing.
#[cfg_attr(all(target_os = "macos", not(test)), allow(dead_code))]
pub struct NoPower;

impl Power for NoPower {
    fn set_mode(&self, _mode: KeepAwake) {}

    fn set_busy(&self, _busy: bool) {}
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{c_char, c_void, CStr};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::Duration;

    use maxhq_protocol::model::KeepAwake;
    use tokio::sync::watch;
    use tracing::{debug, warn};

    use super::{should_hold, Power};

    type CfStringRef = *const c_void;
    type AssertionId = u32;

    /// `kCFStringEncodingUTF8`.
    const UTF8: u32 = 0x0800_0100;
    /// `kIOPMAssertionLevelOn`.
    const LEVEL_ON: u32 = 255;
    /// `kIOPMAssertPreventUserIdleSystemSleep`.
    const PREVENT_IDLE_SLEEP: &CStr = c"PreventUserIdleSystemSleep";
    /// What Activity Monitor and `pmset -g assertions` show.
    const REASON: &CStr = c"MaxHQ is keeping this Mac awake";
    /// `kIOPSTimeRemainingUnlimited`, the estimate on external power and on
    /// a Mac with no battery.
    const UNLIMITED: f64 = -2.0;
    /// How often the `charger` mode looks for external power.
    const CHECK_EVERY: Duration = Duration::from_secs(15);

    #[allow(unsafe_code)]
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            text: *const c_char,
            encoding: u32,
        ) -> CfStringRef;
        fn CFRelease(value: *const c_void);
    }

    #[allow(unsafe_code)]
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(
            kind: CfStringRef,
            level: u32,
            name: CfStringRef,
            id: *mut AssertionId,
        ) -> i32;
        fn IOPMAssertionRelease(id: AssertionId) -> i32;
        fn IOPSGetTimeRemainingEstimate() -> f64;
    }

    /// Holds the assertion when the mode says to.
    #[derive(Default)]
    pub struct Awake {
        /// The mode, which the charger check waits on.
        mode: watch::Sender<KeepAwake>,
        state: Mutex<State>,
    }

    /// What the answer depends on besides the mode, and the assertion it
    /// gave.
    #[derive(Default)]
    struct State {
        busy: bool,
        on_charger: bool,
        held: Option<AssertionId>,
    }

    impl Awake {
        /// A control that keeps checking the charger while the mode needs it.
        pub fn start() -> Arc<Self> {
            let awake = Arc::new(Self::default());
            let weak = Arc::downgrade(&awake);
            crate::spawn_task(
                "the charger check",
                check_charger(awake.mode.subscribe(), CHECK_EVERY, move || {
                    weak.upgrade()
                        .inspect(|awake| awake.read_charger())
                        .is_some()
                }),
            );
            awake
        }

        /// Reads the charger, and acts on it.
        fn read_charger(&self) {
            let on_charger = is_unlimited(time_remaining());
            self.update(|state| state.on_charger = on_charger);
        }

        /// Applies a change, then takes or lets go of the assertion if the
        /// answer changed.
        fn update(&self, change: impl FnOnce(&mut State)) {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            change(&mut state);
            let hold = should_hold(*self.mode.borrow(), state.busy, state.on_charger);
            match (hold, state.held) {
                (true, None) => state.held = create(),
                (false, Some(id)) => {
                    release(id);
                    state.held = None;
                }
                _ => {}
            }
        }
    }

    impl Power for Awake {
        fn set_mode(&self, mode: KeepAwake) {
            self.mode.send_replace(mode);
            self.read_charger();
        }

        fn set_busy(&self, busy: bool) {
            self.update(|state| state.busy = busy);
        }
    }

    impl Drop for Awake {
        fn drop(&mut self) {
            let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
            if let Some(id) = state.held.take() {
                release(id);
            }
        }
    }

    /// Runs `check` every so often while the mode is `charger`, until it
    /// says to stop. In any other mode this only waits for the mode to change.
    async fn check_charger(
        mut mode: watch::Receiver<KeepAwake>,
        every: Duration,
        mut check: impl FnMut() -> bool,
    ) {
        loop {
            tokio::time::sleep(every).await;
            let open = mode
                .wait_for(|mode| *mode == KeepAwake::Charger)
                .await
                .is_ok();
            if !open || !check() {
                return;
            }
        }
    }

    /// IOKit's estimate of the time left on battery.
    #[allow(unsafe_code)]
    fn time_remaining() -> f64 {
        // SAFETY: it takes nothing and only reads.
        unsafe { IOPSGetTimeRemainingEstimate() }
    }

    /// Whether the estimate means there's no battery to run down.
    fn is_unlimited(estimate: f64) -> bool {
        estimate == UNLIMITED
    }

    /// A CoreFoundation string, released when dropped.
    struct CfString(CfStringRef);

    impl CfString {
        #[allow(unsafe_code)]
        fn new(text: &CStr) -> Option<Self> {
            // SAFETY: `text` ends in a zero byte, and a null allocator
            // means the default one.
            let value = unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8) };
            (!value.is_null()).then_some(Self(value))
        }
    }

    impl Drop for CfString {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // SAFETY: the string came from a create call and is released
            // once.
            unsafe { CFRelease(self.0) };
        }
    }

    /// Takes the assertion, or none when IOKit refuses.
    #[allow(unsafe_code)]
    fn create() -> Option<AssertionId> {
        let kind = CfString::new(PREVENT_IDLE_SLEEP)?;
        let name = CfString::new(REASON)?;
        let mut id: AssertionId = 0;
        // SAFETY: both strings live until after the call, and `id` is a
        // valid place for the result.
        let code = unsafe { IOPMAssertionCreateWithName(kind.0, LEVEL_ON, name.0, &mut id) };
        if code != 0 {
            warn!(code, "macOS refused to keep the Mac awake");
            return None;
        }
        debug!("keeping the Mac awake");
        Some(id)
    }

    /// Lets the assertion go.
    #[allow(unsafe_code)]
    fn release(id: AssertionId) {
        // SAFETY: `id` came from a create call and is released once.
        let code = unsafe { IOPMAssertionRelease(id) };
        if code != 0 {
            warn!(code, "couldn't let the Mac sleep again");
        } else {
            debug!("the Mac may sleep again");
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use super::*;

        fn held(awake: &Awake) -> Option<AssertionId> {
            awake.state.lock().unwrap().held
        }

        #[test]
        fn the_assertion_is_taken_once_and_let_go() {
            let awake = Awake::default();
            awake.set_mode(KeepAwake::Working);
            assert!(held(&awake).is_none());
            awake.set_busy(true);
            let first = held(&awake);
            assert!(first.is_some());
            awake.set_busy(true);
            assert_eq!(held(&awake), first);
            awake.set_busy(false);
            assert!(held(&awake).is_none());
        }

        #[test]
        fn a_mode_change_acts_at_once_and_keeps_the_one_assertion() {
            let awake = Awake::default();
            awake.set_mode(KeepAwake::Always);
            let first = held(&awake);
            assert!(first.is_some());
            awake.set_busy(true);
            awake.set_mode(KeepAwake::Working);
            assert_eq!(held(&awake), first);
            awake.set_mode(KeepAwake::Off);
            assert!(held(&awake).is_none());
            awake.set_busy(true);
            assert!(held(&awake).is_none());
        }

        #[test]
        fn unplugging_lets_go_once_no_agent_works() {
            let awake = Awake::default();
            awake.set_mode(KeepAwake::Charger);
            awake.update(|state| state.on_charger = true);
            let first = held(&awake);
            assert!(first.is_some());
            awake.set_busy(true);
            awake.update(|state| state.on_charger = false);
            assert_eq!(held(&awake), first);
            awake.set_busy(false);
            assert!(held(&awake).is_none());
        }

        #[test]
        fn only_an_unlimited_estimate_reads_as_the_charger() {
            assert!(is_unlimited(UNLIMITED));
            // Still working it out, and then the time left.
            assert!(!is_unlimited(-1.0));
            assert!(!is_unlimited(0.0));
            assert!(!is_unlimited(7200.0));
            let now = time_remaining();
            assert!(now == UNLIMITED || now == -1.0 || now >= 0.0, "{now}");
        }

        #[tokio::test]
        async fn the_charger_is_checked_only_while_the_mode_needs_it() {
            let every = Duration::from_millis(2);
            let (mode, watched) = watch::channel(KeepAwake::Charger);
            let checks = Arc::new(AtomicUsize::new(0));
            let counted = checks.clone();
            let task = tokio::spawn(check_charger(watched, every, move || {
                counted.fetch_add(1, Ordering::Relaxed) < 3
            }));
            while checks.load(Ordering::Relaxed) < 2 {
                tokio::time::sleep(every).await;
            }

            mode.send_replace(KeepAwake::Off);
            let before = checks.load(Ordering::Relaxed);
            tokio::time::sleep(every * 10).await;
            assert_eq!(checks.load(Ordering::Relaxed), before);

            mode.send_replace(KeepAwake::Charger);
            task.await.unwrap();
            assert_eq!(checks.load(Ordering::Relaxed), 4);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_mode_holds_when_it_says() {
        use KeepAwake::{Always, Charger, Off, Working};
        for (mode, busy, on_charger, holds) in [
            (Charger, false, false, false),
            (Charger, true, false, true),
            (Charger, false, true, true),
            (Charger, true, true, true),
            (Always, false, false, true),
            (Always, true, false, true),
            (Always, false, true, true),
            (Always, true, true, true),
            (Working, false, false, false),
            (Working, true, false, true),
            (Working, false, true, false),
            (Working, true, true, true),
            (Off, false, false, false),
            (Off, true, false, false),
            (Off, false, true, false),
            (Off, true, true, false),
        ] {
            assert_eq!(
                should_hold(mode, busy, on_charger),
                holds,
                "{mode:?}, busy {busy}, on the charger {on_charger}"
            );
        }
    }
}
