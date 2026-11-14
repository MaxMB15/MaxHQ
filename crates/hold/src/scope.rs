// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The systemd scope a holder runs in, and clearing one that an earlier
//! holder of the session left behind.
//!
//! Each holder's scope is named for its session, and systemd won't start a
//! unit whose name is still loaded. A scope stays loaded while anything
//! runs in it, and for a moment after it ends. So before a spawn starts a
//! scope, it looks at the unit with that name:
//!
//! - When there's none, the spawn goes ahead.
//! - When the unit failed, `systemctl reset-failed` lets systemd unload it.
//!   That stops nothing.
//! - When it ended, systemd unloads it soon, so the spawn waits.
//! - When something still runs in it, the spawn waits a few seconds for it
//!   to end by itself, then stops the unit, then kills it.
//!
//! A spawn only ever looks at the unit after it has made the session's
//! folder itself, so no holder can be serving the session from that unit.
//! Whatever runs there is left from a holder whose folder has gone, and
//! the session's lock is checked again before each stop or kill.

use std::process::Stdio;
use std::time::Duration;

use maxhq_protocol::ids::SessionId;
use tokio::process::Command;
use tokio::time::Instant;

/// How long `systemctl` has to answer.
const SYSTEMCTL_WAIT: Duration = Duration::from_secs(10);

/// The name of the scope unit that `session`'s holder runs in, such as
/// `maxhq-hold-0199f0a2-7c3e-7a10-9b4d-1234567890ab.scope`.
pub fn unit_name(session: SessionId) -> String {
    format!("{}.scope", unit_base(session))
}

/// The name `systemd-run --unit` takes, without `.scope`.
pub(crate) fn unit_base(session: SessionId) -> String {
    format!("maxhq-hold-{session}")
}

/// What systemd says about a unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnitState {
    /// There's no unit with the name.
    Missing,
    /// It ended and systemd hasn't unloaded it yet.
    Inactive,
    /// It failed, which keeps it loaded until it's reset.
    Failed,
    /// Something still runs in it, or it's starting or stopping.
    Running,
}

/// How far clearing a unit has gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Waiting for the unit to end by itself.
    Watching,
    /// The unit was asked to stop.
    Stopping,
    /// The unit's processes were sent SIGKILL.
    Killing,
}

/// The next step in clearing a unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// The name is free.
    Go,
    /// Look again in a moment.
    Wait,
    /// Run `systemctl reset-failed`.
    Reset,
    /// Run `systemctl stop`.
    Stop,
    /// Send SIGKILL to everything in the unit.
    Kill,
    /// Give up, for this reason.
    Refuse(String),
}

/// The next step for a unit in `state`, at `stage`. `waited` says whether
/// the stage has had its time, and `holder_alive` whether the session's
/// lock is held, which means a holder of the session runs.
pub(crate) fn decide(state: UnitState, stage: Stage, waited: bool, holder_alive: bool) -> Step {
    match (state, stage, waited) {
        (UnitState::Missing, _, _) => Step::Go,
        (UnitState::Failed | UnitState::Inactive, _, true) => {
            Step::Refuse("it stays loaded after it ended".into())
        }
        (UnitState::Failed, _, false) => Step::Reset,
        (UnitState::Inactive, _, false) => Step::Wait,
        (UnitState::Running, _, _) if holder_alive => {
            Step::Refuse("a holder for the session runs in it".into())
        }
        (UnitState::Running, _, false) => Step::Wait,
        (UnitState::Running, Stage::Watching, true) => Step::Stop,
        (UnitState::Running, Stage::Stopping, true) => Step::Kill,
        (UnitState::Running, Stage::Killing, true) => {
            Step::Refuse("it still runs after SIGKILL".into())
        }
    }
}

/// How long each stage of clearing a unit lasts.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Waits {
    /// How long a unit has to end by itself, or to unload once ended.
    pub settle: Duration,
    /// How long a stopped unit has to end.
    pub stop: Duration,
    /// How long a killed unit has to end.
    pub kill: Duration,
    /// The pause between looks.
    pub poll: Duration,
}

impl Waits {
    /// The waits a real spawn uses. A holder needs about 3 seconds to stop
    /// its CLI after SIGTERM.
    pub const SPAWN: Self = Self {
        settle: Duration::from_secs(5),
        stop: Duration::from_secs(10),
        kill: Duration::from_secs(5),
        poll: Duration::from_millis(100),
    };
}

/// What clearing a unit needs from systemd. The real one runs `systemctl`.
pub(crate) trait Units {
    /// The unit's state.
    async fn state(&self, unit: &str) -> Result<UnitState, String>;
    /// Runs `systemctl reset-failed` on it.
    async fn reset(&self, unit: &str) -> Result<(), String>;
    /// Asks it to stop, without waiting for it.
    async fn stop(&self, unit: &str) -> Result<(), String>;
    /// Sends SIGKILL to everything in it.
    async fn kill(&self, unit: &str) -> Result<(), String>;
}

/// Makes sure no unit called `unit` is loaded, so a scope with that name
/// can start. `holder_alive` says whether a holder of the session runs.
pub(crate) async fn clear(
    units: &impl Units,
    unit: &str,
    holder_alive: impl Fn() -> bool,
    waits: Waits,
) -> Result<(), String> {
    let mut stage = Stage::Watching;
    let mut since = Instant::now();
    loop {
        let state = units.state(unit).await?;
        let limit = match stage {
            Stage::Watching => waits.settle,
            Stage::Stopping => waits.stop,
            Stage::Killing => waits.kill,
        };
        let waited = since.elapsed() >= limit;
        let alive = state == UnitState::Running && holder_alive();
        match decide(state, stage, waited, alive) {
            Step::Go => return Ok(()),
            Step::Wait => {}
            Step::Reset => units.reset(unit).await?,
            Step::Stop => {
                units.stop(unit).await?;
                stage = Stage::Stopping;
                since = Instant::now();
            }
            Step::Kill => {
                units.kill(unit).await?;
                stage = Stage::Killing;
                since = Instant::now();
            }
            Step::Refuse(reason) => return Err(reason),
        }
        tokio::time::sleep(waits.poll).await;
    }
}

/// `systemctl`, on the user's manager or the system's.
pub(crate) struct Systemctl {
    /// Whether to talk to the user's manager.
    pub user: bool,
}

impl Systemctl {
    async fn run(&self, args: &[&str]) -> Result<String, String> {
        let mut command = Command::new("systemctl");
        if self.user {
            command.arg("--user");
        }
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let output = match tokio::time::timeout(SYSTEMCTL_WAIT, command.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => return Err(format!("couldn't run systemctl: {err}")),
            Err(_) => return Err("systemctl didn't answer in time".into()),
        };
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            let said = String::from_utf8_lossy(&output.stderr);
            Err(format!(
                "systemctl {} failed with {}: {}",
                args.first().copied().unwrap_or_default(),
                output.status,
                said.trim()
            ))
        }
    }
}

impl Units for Systemctl {
    async fn state(&self, unit: &str) -> Result<UnitState, String> {
        let shown = self
            .run(&[
                "show",
                "--property=LoadState",
                "--property=ActiveState",
                "--",
                unit,
            ])
            .await?;
        Ok(parse_state(&shown))
    }

    async fn reset(&self, unit: &str) -> Result<(), String> {
        self.run(&["reset-failed", "--", unit]).await.map(drop)
    }

    async fn stop(&self, unit: &str) -> Result<(), String> {
        self.run(&["stop", "--no-block", "--", unit])
            .await
            .map(drop)
    }

    async fn kill(&self, unit: &str) -> Result<(), String> {
        self.run(&["kill", "--signal=SIGKILL", "--", unit])
            .await
            .map(drop)
    }
}

/// The state in what `systemctl show` printed.
fn parse_state(shown: &str) -> UnitState {
    let value = |key: &str| {
        shown
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .unwrap_or_default()
            .trim()
    };
    match (value("LoadState"), value("ActiveState")) {
        ("not-found" | "", _) => UnitState::Missing,
        (_, "inactive") => UnitState::Inactive,
        (_, "failed") => UnitState::Failed,
        _ => UnitState::Running,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn units_are_named_for_the_session() {
        let session: SessionId = "0199f0a2-7c3e-7a10-9b4d-1234567890ab".parse().unwrap();
        assert_eq!(
            unit_name(session),
            "maxhq-hold-0199f0a2-7c3e-7a10-9b4d-1234567890ab.scope"
        );
        assert_eq!(
            unit_base(session),
            "maxhq-hold-0199f0a2-7c3e-7a10-9b4d-1234567890ab"
        );
    }

    #[test]
    fn show_output_parses() {
        let missing = "LoadState=not-found\nActiveState=inactive\n";
        assert_eq!(parse_state(missing), UnitState::Missing);
        assert_eq!(parse_state(""), UnitState::Missing);
        let shown = |active: &str| format!("LoadState=loaded\nActiveState={active}\n");
        assert_eq!(parse_state(&shown("inactive")), UnitState::Inactive);
        assert_eq!(parse_state(&shown("failed")), UnitState::Failed);
        for active in ["active", "activating", "deactivating"] {
            assert_eq!(parse_state(&shown(active)), UnitState::Running, "{active}");
        }
    }

    #[test]
    fn a_live_holder_is_never_stopped() {
        for stage in [Stage::Watching, Stage::Stopping, Stage::Killing] {
            for waited in [false, true] {
                let step = decide(UnitState::Running, stage, waited, true);
                assert!(
                    matches!(step, Step::Refuse(_)),
                    "{stage:?} {waited}: {step:?}"
                );
            }
        }
    }

    #[test]
    fn each_state_has_its_step() {
        use Stage::{Killing, Stopping, Watching};
        use UnitState::{Failed, Inactive, Missing, Running};
        assert_eq!(decide(Missing, Watching, false, false), Step::Go);
        assert_eq!(decide(Missing, Killing, true, true), Step::Go);
        assert_eq!(decide(Failed, Watching, false, false), Step::Reset);
        assert_eq!(decide(Inactive, Stopping, false, false), Step::Wait);
        assert!(matches!(
            decide(Failed, Watching, true, false),
            Step::Refuse(_)
        ));
        assert_eq!(decide(Running, Watching, false, false), Step::Wait);
        assert_eq!(decide(Running, Watching, true, false), Step::Stop);
        assert_eq!(decide(Running, Stopping, false, false), Step::Wait);
        assert_eq!(decide(Running, Stopping, true, false), Step::Kill);
        assert!(matches!(
            decide(Running, Killing, true, false),
            Step::Refuse(_)
        ));
    }

    /// A unit that goes through `states` one look at a time, staying in the
    /// last, and records what was done to it.
    struct Fake {
        states: Mutex<Vec<UnitState>>,
        done: Mutex<Vec<&'static str>>,
        /// The state a stop or kill moves the unit to.
        on_stop: Option<UnitState>,
    }

    impl Fake {
        fn new(mut states: Vec<UnitState>, on_stop: Option<UnitState>) -> Self {
            states.reverse();
            Self {
                states: Mutex::new(states),
                done: Mutex::new(Vec::new()),
                on_stop,
            }
        }

        fn done(&self) -> Vec<&'static str> {
            self.done.lock().unwrap().clone()
        }

        fn act(&self, what: &'static str, then: Option<UnitState>) {
            self.done.lock().unwrap().push(what);
            if let Some(then) = then {
                *self.states.lock().unwrap() = vec![then];
            }
        }
    }

    impl Units for Fake {
        async fn state(&self, _unit: &str) -> Result<UnitState, String> {
            let mut states = self.states.lock().unwrap();
            Ok(if states.len() > 1 {
                states.pop().unwrap()
            } else {
                states[0]
            })
        }

        async fn reset(&self, _unit: &str) -> Result<(), String> {
            self.act("reset", Some(UnitState::Missing));
            Ok(())
        }

        async fn stop(&self, _unit: &str) -> Result<(), String> {
            self.act("stop", self.on_stop);
            Ok(())
        }

        async fn kill(&self, _unit: &str) -> Result<(), String> {
            self.act("kill", self.on_stop);
            Ok(())
        }
    }

    const QUICK: Waits = Waits {
        settle: Duration::from_millis(500),
        stop: Duration::from_millis(50),
        kill: Duration::from_millis(50),
        poll: Duration::from_millis(1),
    };

    fn clear_now(units: &Fake, holder_alive: bool) -> Result<(), String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(clear(units, "maxhq-hold-x.scope", || holder_alive, QUICK))
    }

    #[test]
    fn a_missing_unit_needs_nothing() {
        let units = Fake::new(vec![UnitState::Missing], None);
        clear_now(&units, false).unwrap();
        assert!(units.done().is_empty());
    }

    #[test]
    fn a_failed_unit_is_reset_and_nothing_stopped() {
        let units = Fake::new(vec![UnitState::Failed], None);
        clear_now(&units, false).unwrap();
        assert_eq!(units.done(), ["reset"]);
    }

    #[test]
    fn a_unit_that_ends_by_itself_is_left_to_end() {
        let states = vec![
            UnitState::Running,
            UnitState::Running,
            UnitState::Inactive,
            UnitState::Missing,
        ];
        let units = Fake::new(states, None);
        clear_now(&units, false).unwrap();
        assert!(units.done().is_empty());
    }

    #[test]
    fn a_unit_that_stays_is_stopped_then_killed() {
        let units = Fake::new(vec![UnitState::Running], Some(UnitState::Inactive));
        assert!(clear_now(&units, false).is_err());
        assert_eq!(units.done(), ["stop"]);

        let units = Fake::new(vec![UnitState::Running], Some(UnitState::Missing));
        clear_now(&units, false).unwrap();
        assert_eq!(units.done(), ["stop"]);

        let units = Fake::new(vec![UnitState::Running], None);
        let err = clear_now(&units, false).unwrap_err();
        assert!(err.contains("SIGKILL"), "{err}");
        assert_eq!(units.done(), ["stop", "kill"]);
    }

    #[test]
    fn a_unit_with_a_live_holder_is_left_alone() {
        let units = Fake::new(vec![UnitState::Running], Some(UnitState::Missing));
        let err = clear_now(&units, true).unwrap_err();
        assert!(err.contains("holder"), "{err}");
        assert!(units.done().is_empty());
    }
}
