// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! On macOS, a `maxd` that came in the MaxHQ app notices when you delete
//! the app, and then stops and uninstalls itself, so nothing of it keeps
//! running. It only watches when the LaunchAgent runs this very program
//! from inside an app.
//!
//! The app has to be missing on every check for [`GONE_AFTER`], so an
//! update that replaces it doesn't count. Time asleep doesn't count either.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub use maxhq_core::service::program_in;

/// How often the app is looked for.
pub const CHECK_EVERY: Duration = Duration::from_secs(60);

/// How long the app must be missing before `maxd` uninstalls itself.
pub const GONE_AFTER: Duration = Duration::from_secs(5 * 60);

/// Waits until the app this `maxd` came in has been deleted. Off macOS, or
/// when the LaunchAgent doesn't run this program from an app, it never
/// returns.
pub async fn app_deleted() {
    #[cfg(target_os = "macos")]
    if let Some(exe) = watched_here() {
        let mut ticks = tokio::time::interval(CHECK_EVERY);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut missing = Missing::default();
        loop {
            ticks.tick().await;
            if missing.check(Instant::now(), app_there(&exe)) {
                return;
            }
        }
    }
    std::future::pending().await
}

/// This program, when it's the one to watch.
#[cfg(target_os = "macos")]
fn watched_here() -> Option<PathBuf> {
    let exe = std::env::current_exe().and_then(fs::canonicalize).ok()?;
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    watched(&super::launch_agent_file(Path::new(&home)), &exe).then_some(exe)
}

/// Whether `exe` is to be watched: it's `maxd` inside an app, and the
/// LaunchAgent `file` runs it, by that path or through a link to it.
pub fn watched(file: &Path, exe: &Path) -> bool {
    if !in_app(exe) {
        return false;
    }
    let Some(program) = fs::read_to_string(file)
        .ok()
        .and_then(|plist| program_in(&plist))
    else {
        return false;
    };
    program == exe || fs::canonicalize(&program).is_ok_and(|program| program == exe)
}

/// Whether `exe` is `<name>.app/Contents/MacOS/maxd`.
pub fn in_app(exe: &Path) -> bool {
    let named = |path: Option<&Path>, name: &str| {
        path.and_then(Path::file_name)
            .is_some_and(|file_name| file_name == name)
    };
    let macos = exe.parent();
    let contents = macos.and_then(Path::parent);
    let app = contents.and_then(Path::parent);
    named(Some(exe), "maxd")
        && named(macos, "MacOS")
        && named(contents, "Contents")
        && app
            .and_then(Path::extension)
            .is_some_and(|extension| extension == "app")
}

/// Whether the app `exe` is in still looks there. When the folder that held
/// the app has gone too, as when a disk image is ejected, it wasn't
/// deleted, so that counts as there. So does a check that fails.
pub fn app_there(exe: &Path) -> bool {
    if exe.try_exists().unwrap_or(true) {
        return true;
    }
    let holder = exe.ancestors().nth(4);
    !holder.is_some_and(|holder| holder.try_exists().unwrap_or(true))
}

/// How long the app has been missing.
#[derive(Debug, Default)]
pub struct Missing {
    /// The first check that found it missing, since it was last there.
    since: Option<Instant>,
}

impl Missing {
    /// Records a check at `now` that found the app `there` or not. Answers
    /// true once it has been missing on every check for [`GONE_AFTER`].
    pub fn check(&mut self, now: Instant, there: bool) -> bool {
        if there {
            self.since = None;
            return false;
        }
        let since = *self.since.get_or_insert(now);
        now.duration_since(since) >= GONE_AFTER
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::plist;
    use crate::testing::TempDir;

    #[test]
    fn the_program_reads_back_from_the_plist() {
        let exe = Path::new("/Applications/Max & Co <beta>.app/Contents/MacOS/maxd");
        let file = plist(exe, "/usr/bin:/bin", Path::new("/tmp/maxd.log"));
        assert_eq!(program_in(&file).as_deref(), Some(exe));
        assert_eq!(program_in("<plist><dict></dict></plist>"), None);
    }

    #[test]
    fn only_maxd_inside_an_app_is_in_one() {
        assert!(in_app(Path::new(
            "/Applications/MaxHQ.app/Contents/MacOS/maxd"
        )));
        assert!(in_app(Path::new(
            "/Users/me/Apps/Other.app/Contents/MacOS/maxd"
        )));
        assert!(!in_app(Path::new("/Users/me/.local/bin/maxd")));
        assert!(!in_app(Path::new(
            "/Applications/MaxHQ.app/Contents/MacOS/maxhq"
        )));
        assert!(!in_app(Path::new(
            "/Applications/MaxHQ/Contents/MacOS/maxd"
        )));
        assert!(!in_app(Path::new(
            "/Applications/MaxHQ.app/Contents/Resources/maxd"
        )));
        assert!(!in_app(Path::new("MacOS/maxd")));
    }

    /// Makes `<dir>/Apps/MaxHQ.app/Contents/MacOS/maxd` and gives its path.
    fn app(dir: &TempDir) -> PathBuf {
        let exe = dir.path().join("Apps/MaxHQ.app/Contents/MacOS/maxd");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, "").unwrap();
        fs::canonicalize(exe).unwrap()
    }

    #[test]
    fn maxd_is_watched_only_when_the_launch_agent_runs_it() {
        let dir = TempDir::new();
        let exe = app(&dir);
        let file = dir.path().join("agent.plist");
        let write = |program: &Path| {
            fs::write(&file, plist(program, "/usr/bin", Path::new("/tmp/log"))).unwrap();
        };
        assert!(!watched(&file, &exe), "there's no LaunchAgent yet");

        write(&exe);
        assert!(watched(&file, &exe));

        // A link to the app's maxd, the way ~/.local/bin/maxd can be one.
        let link = dir.path().join("maxd");
        std::os::unix::fs::symlink(&exe, &link).unwrap();
        write(&link);
        assert!(watched(&file, &exe));

        write(Path::new("/Users/me/.local/lib/maxhq/1.0.0/maxd"));
        assert!(!watched(&file, &exe), "the agent runs another maxd");

        let outside = dir.path().join("bin/maxd");
        fs::create_dir_all(outside.parent().unwrap()).unwrap();
        fs::write(&outside, "").unwrap();
        write(&outside);
        assert!(!watched(&file, &outside), "this maxd isn't in an app");
    }

    #[test]
    fn a_deleted_app_is_missing_but_an_ejected_disk_is_not() {
        let dir = TempDir::new();
        let exe = app(&dir);
        assert!(app_there(&exe));

        fs::remove_dir_all(dir.path().join("Apps/MaxHQ.app")).unwrap();
        assert!(!app_there(&exe));

        // The folder that held the app went too, like an ejected volume.
        fs::remove_dir_all(dir.path().join("Apps")).unwrap();
        assert!(app_there(&exe));
    }

    #[test]
    fn the_app_counts_as_deleted_after_five_minutes_missing() {
        let start = Instant::now();
        let at = |minutes: u64| start + Duration::from_secs(minutes * 60);
        let mut missing = Missing::default();
        assert!(!missing.check(at(0), true));
        assert!(!missing.check(at(1), false));
        assert!(!missing.check(at(5), false), "missing for four minutes");
        assert!(missing.check(at(6), false), "missing for five minutes");

        // Back again, so the count starts over.
        let mut missing = Missing::default();
        assert!(!missing.check(at(0), false));
        assert!(!missing.check(at(4), false));
        assert!(!missing.check(at(5), true));
        assert!(!missing.check(at(6), false));
        assert!(!missing.check(at(10), false));
        assert!(missing.check(at(11), false));
    }
}
