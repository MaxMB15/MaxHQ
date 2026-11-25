// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Keeping the device awake, per decision A-11. A sleeping machine stops
//! `maxd`, so the device's settings say when it holds the system awake: an
//! IOKit assertion that stops idle sleep on a Mac, a logind inhibitor on
//! Linux. The display still turns off, and closing the lid still sleeps
//! the machine. Other systems don't sleep under an agent, so there it does
//! nothing.
//!
//! When to hold is decided here, once. Each system's module only takes the
//! hold and says whether the machine runs on the charger.

use std::sync::Arc;

use maxhq_protocol::model::KeepAwake;

// Linux's module also builds in every test run, so its reading of the
// power supplies is tested on any system.
#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;

/// Holds the device awake when its mode says to.
pub trait Power: Send + Sync {
    /// Says which mode applies. It takes effect at once.
    fn set_mode(&self, mode: KeepAwake);

    /// Says whether any agent here is busy. It's called after every input,
    /// so it does nothing unless the answer changed.
    fn set_busy(&self, busy: bool);
}

/// Whether to hold the device awake in this mode.
#[cfg_attr(
    all(not(any(target_os = "macos", target_os = "linux")), not(test)),
    allow(dead_code)
)]
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
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        awake::Awake::start(platform::holder())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Arc::new(NoPower)
    }
}

/// Power control that does nothing.
#[cfg_attr(
    all(any(target_os = "macos", target_os = "linux"), not(test)),
    allow(dead_code)
)]
pub struct NoPower;

impl Power for NoPower {
    fn set_mode(&self, _mode: KeepAwake) {}

    fn set_busy(&self, _busy: bool) {}
}

/// The hold and the charger check every system that can hold itself awake
/// shares. The tests build it everywhere, against a fake system.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
mod awake {
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::Duration;

    use maxhq_protocol::model::KeepAwake;
    use tokio::sync::watch;
    use tracing::debug;

    use super::{should_hold, Power};

    /// What a system gives [`Awake`]: a way to hold it awake, and whether
    /// it runs on the charger.
    pub(super) trait Holder: Send + Sync + 'static {
        /// Keeps the system awake until it's dropped.
        type Hold: Send;

        /// What the logs call the machine, such as "the Mac".
        const MACHINE: &'static str;

        /// How often the `charger` mode looks for external power.
        const CHECK_EVERY: Duration;

        /// Takes a hold, or none when the system refuses. The system's
        /// module logs why.
        fn take(&self) -> Option<Self::Hold>;

        /// Whether the machine runs on external power, or has no battery.
        fn on_charger(&self) -> bool;
    }

    /// Holds the system awake when the mode says to.
    pub(super) struct Awake<H: Holder> {
        pub(super) holder: H,
        /// The mode, which the charger check waits on.
        mode: watch::Sender<KeepAwake>,
        pub(super) state: Mutex<State<H::Hold>>,
    }

    /// What the answer depends on besides the mode, and the hold it gave,
    /// which dropping lets go.
    pub(super) struct State<T> {
        busy: bool,
        on_charger: bool,
        pub(super) held: Option<T>,
    }

    impl<H: Holder> Awake<H> {
        /// A control that holds nothing yet and checks nothing by itself.
        pub(super) fn new(holder: H) -> Self {
            Self {
                holder,
                mode: watch::Sender::default(),
                state: Mutex::new(State {
                    busy: false,
                    on_charger: false,
                    held: None,
                }),
            }
        }

        /// A control that keeps checking the charger while the mode needs it.
        #[cfg_attr(
            all(test, not(any(target_os = "macos", target_os = "linux"))),
            allow(dead_code)
        )]
        pub(super) fn start(holder: H) -> Arc<Self> {
            let awake = Arc::new(Self::new(holder));
            let weak = Arc::downgrade(&awake);
            crate::spawn_task(
                "the charger check",
                check_charger(awake.mode.subscribe(), H::CHECK_EVERY, move || {
                    weak.upgrade()
                        .inspect(|awake| awake.read_charger())
                        .is_some()
                }),
            );
            awake
        }

        /// Reads the charger, and acts on it.
        pub(super) fn read_charger(&self) {
            let on_charger = self.holder.on_charger();
            self.update(|state| state.on_charger = on_charger);
        }

        /// Applies a change, then takes or lets go of the hold if the
        /// answer changed.
        fn update(&self, change: impl FnOnce(&mut State<H::Hold>)) {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            change(&mut state);
            let hold = should_hold(*self.mode.borrow(), state.busy, state.on_charger);
            match (hold, &state.held) {
                (true, None) => state.held = self.holder.take(),
                (false, Some(_)) => {
                    state.held = None;
                    debug!("{} may sleep again", H::MACHINE);
                }
                _ => {}
            }
        }
    }

    impl<H: Holder> Power for Awake<H> {
        fn set_mode(&self, mode: KeepAwake) {
            self.mode.send_replace(mode);
            self.read_charger();
        }

        fn set_busy(&self, busy: bool) {
            self.update(|state| state.busy = busy);
        }
    }

    /// Runs `check` every so often while the mode is `charger`, until it
    /// says to stop. In any other mode this only waits for the mode to
    /// change.
    pub(super) async fn check_charger(
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
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
    use std::time::Duration;

    use tokio::sync::watch;

    use super::awake::{check_charger, Awake, Holder};
    use super::*;

    /// A system whose holds are numbers counted up from 1, and whose
    /// charger the test plugs in and out.
    #[derive(Default)]
    struct Fake {
        taken: AtomicU32,
        on_charger: AtomicBool,
    }

    impl Holder for Fake {
        type Hold = u32;
        const MACHINE: &'static str = "the test machine";
        const CHECK_EVERY: Duration = Duration::from_millis(2);

        fn take(&self) -> Option<u32> {
            Some(self.taken.fetch_add(1, Ordering::Relaxed) + 1)
        }

        fn on_charger(&self) -> bool {
            self.on_charger.load(Ordering::Relaxed)
        }
    }

    fn held(awake: &Awake<Fake>) -> Option<u32> {
        awake.state.lock().unwrap().held
    }

    fn plug(awake: &Awake<Fake>, on_charger: bool) {
        awake.holder.on_charger.store(on_charger, Ordering::Relaxed);
        awake.read_charger();
    }

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

    #[test]
    fn the_hold_is_taken_once_and_let_go() {
        let awake = Awake::new(Fake::default());
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
    fn a_mode_change_acts_at_once_and_keeps_the_one_hold() {
        let awake = Awake::new(Fake::default());
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
        let awake = Awake::new(Fake::default());
        awake.set_mode(KeepAwake::Charger);
        plug(&awake, true);
        let first = held(&awake);
        assert!(first.is_some());
        awake.set_busy(true);
        plug(&awake, false);
        assert_eq!(held(&awake), first);
        awake.set_busy(false);
        assert!(held(&awake).is_none());
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
