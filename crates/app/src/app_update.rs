// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The app's own update: whether this copy of the app offers one, the
//! setting that says whether it looks for one by itself, and what becomes
//! of this Mac's `maxd` afterwards. The shell does the looking and the
//! installing; these are the rules it follows.
//!
//! This Mac's `maxd` ships inside the app, and its LaunchAgent runs it from
//! there. An update swaps the app's files, but the `maxd` already running
//! keeps the old program until something restarts it. So once the app
//! starts, it compares the build this Mac's `maxd` reports with the one
//! beside the app, and restarts the service onto the new one when they
//! differ. Agents keep running across that restart.

use std::path::{Path, PathBuf};
use std::time::Duration;

use maxhq_protocol::time::Timestamp;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::devices::Connector;
use crate::fleet::{machines, Machine, DEV_BUNDLE};
use crate::registry::{local_link, Registry, RegistryError};
use crate::App;

/// The setting the update settings are kept under.
const PREFS: &str = "app_updates";

/// How long the app waits after it starts before it first looks for an
/// update, so the look doesn't slow the start.
pub const FIRST_CHECK_AFTER: Duration = Duration::from_secs(20);

/// How often the app looks for an update while it runs.
pub const CHECK_EVERY: Duration = Duration::from_secs(4 * 60 * 60);

/// How long the app waits for this Mac's `maxd` to say hello before it
/// gives up on bringing it onto the bundled build.
pub const LOCAL_MAXD_WAIT: Duration = Duration::from_secs(120);

/// How often the view is read while the app waits for that hello.
const LOCAL_MAXD_POLL: Duration = Duration::from_secs(2);

/// Why this copy of the app doesn't offer updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum NoUpdates {
    /// A debug build, or one run from the source tree. A new build comes
    /// from the source instead.
    FromSource,
    /// It runs from a disk image, which goes once you eject it.
    DiskImage,
    /// macOS moved it aside, because it was opened where it was downloaded.
    Translocated,
    /// It's outside the Applications folder, where an update can't be sure
    /// to replace it.
    NotInApplications,
}

/// Whether the app whose program is at `exe` offers updates. `debug` says
/// it's a debug build.
pub fn offered(exe: &Path, debug: bool) -> Result<(), NoUpdates> {
    if debug {
        return Err(NoUpdates::FromSource);
    }
    let Some(app) = app_of(exe) else {
        return Err(NoUpdates::FromSource);
    };
    if app.file_name().is_some_and(|name| name == DEV_BUNDLE) {
        return Err(NoUpdates::FromSource);
    }
    if let Some(moved) = moved_aside(exe) {
        return Err(moved);
    }
    let in_applications = app
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|folder| folder == "Applications");
    if in_applications {
        Ok(())
    } else {
        Err(NoUpdates::NotInApplications)
    }
}

/// Whether `path` is somewhere that goes away: a disk image, or a copy
/// macOS moved aside. A login service can't keep pointing there, and an
/// update can't replace it.
pub fn moved_aside(path: &Path) -> Option<NoUpdates> {
    let text = path.to_string_lossy();
    if text.contains("/AppTranslocation/") {
        Some(NoUpdates::Translocated)
    } else if text.starts_with("/Volumes/") {
        Some(NoUpdates::DiskImage)
    } else {
        None
    }
}

/// The `.app` folder `exe` runs from, when it's `<name>.app/Contents/MacOS/<exe>`.
fn app_of(exe: &Path) -> Option<&Path> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    let named = |path: &Path, name: &str| path.file_name().is_some_and(|found| found == name);
    (named(macos, "MacOS")
        && named(contents, "Contents")
        && app.extension().is_some_and(|extension| extension == "app"))
    .then_some(app)
}

/// Your settings for the app's updates, kept with this Mac's other app
/// settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(default)]
pub struct UpdatePrefs {
    /// Whether the app looks for an update when it starts and every few
    /// hours after.
    pub auto_check: bool,
}

impl Default for UpdatePrefs {
    fn default() -> Self {
        Self { auto_check: true }
    }
}

impl UpdatePrefs {
    /// The settings saved in `registry`, or the defaults.
    pub fn load(registry: &Registry) -> Self {
        match registry.setting(PREFS) {
            Ok(saved) => saved.unwrap_or_default(),
            Err(error) => {
                tracing::warn!(%error, "couldn't read the update settings");
                Self::default()
            }
        }
    }

    /// Saves the settings in `registry`.
    pub fn save(&self, registry: &Registry) -> Result<(), RegistryError> {
        registry.set_setting(PREFS, self)
    }
}

/// Where the app's own update stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UpdateStatus {
    /// This copy doesn't offer updates, for `reason`.
    Unavailable {
        /// Why not.
        reason: NoUpdates,
    },
    /// It hasn't looked yet.
    Idle,
    /// It's looking now.
    Checking,
    /// It looked, and this is the newest version.
    UpToDate {
        /// When it looked.
        checked_at: Timestamp,
    },
    /// A newer version is out.
    Available {
        /// The newer version, such as `0.2.0`.
        version: String,
    },
    /// The newer version is on its way.
    Downloading {
        /// The version.
        version: String,
        /// Bytes so far.
        #[specta(type = specta_typescript::Number)]
        downloaded: u64,
        /// All of it, when the server says.
        #[specta(type = Option<specta_typescript::Number>)]
        total: Option<u64>,
    },
    /// It's downloaded and going into place. The app restarts next.
    Installing {
        /// The version.
        version: String,
    },
    /// Looking or installing failed.
    Failed {
        /// Why, as the updater said.
        reason: String,
        /// The newer version, when it failed installing one, so you can
        /// try again.
        version: Option<String>,
    },
}

/// What becomes of this Mac's `maxd` once the app starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalMaxd {
    /// Restart its service, so it runs the bundled build.
    Restart,
    /// Look again shortly: it hasn't said hello yet, or it's winding down.
    Wait,
    /// Leave it, for the reason given, for the log.
    Leave(&'static str),
}

/// What the app knows about this Mac's `maxd` when it decides.
#[derive(Debug, Clone, Copy)]
pub struct LocalFacts<'a> {
    /// The app runs from the source tree, where you restart `maxd` with
    /// Start maxd yourself.
    pub dev_tree: bool,
    /// The program the LaunchAgent runs, if there is one.
    pub service_runs: Option<&'a Path>,
    /// The `maxd` beside the app.
    pub bundled: &'a Path,
    /// That `maxd`'s build, if the app could read it.
    pub bundled_build: Option<&'a str>,
    /// This Mac, as the view has it.
    pub machine: Option<&'a Machine>,
}

/// Whether to restart this Mac's `maxd` onto the build beside the app. Only
/// when the LaunchAgent runs that very program, so an app that isn't the
/// one installing `maxd` leaves it be, and only once its `maxd` says which
/// build it runs.
pub fn local_maxd(facts: LocalFacts<'_>) -> LocalMaxd {
    if facts.dev_tree {
        return LocalMaxd::Leave("the app runs from the source tree");
    }
    let Some(program) = facts.service_runs else {
        return LocalMaxd::Leave("no LaunchAgent runs maxd");
    };
    if !same_file(program, facts.bundled) {
        return LocalMaxd::Leave("the LaunchAgent runs another maxd");
    }
    let Some(bundled_build) = facts.bundled_build else {
        return LocalMaxd::Leave("the bundled maxd couldn't be read");
    };
    let Some(machine) = facts.machine.filter(|machine| machine.connected) else {
        return LocalMaxd::Wait;
    };
    if machine.winding_down {
        return LocalMaxd::Wait;
    }
    if machine.build.as_deref() == Some(bundled_build) {
        return LocalMaxd::Leave("it runs the bundled build");
    }
    LocalMaxd::Restart
}

/// Whether two paths name one file, through links too.
fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

/// The program the LaunchAgent in `home` runs, if there's one.
pub fn service_program(home: &Path) -> Option<PathBuf> {
    let file = maxhq_core::service::launch_agent_file(home);
    let plist = std::fs::read_to_string(file).ok()?;
    maxhq_core::service::program_in(&plist)
}

impl<C: Connector> App<C> {
    /// Brings this Mac's `maxd` onto the build beside the app, once it says
    /// hello, when the LaunchAgent runs that `maxd` and it runs another
    /// build: after an update, or after you put a new copy of the app in
    /// place by hand. It restarts the service the way Restart maxd does,
    /// and agents keep running across that. It decides once each start.
    pub async fn keep_local_maxd_current(&self) {
        let Some(bundled) = crate::devices::bundled_maxd() else {
            return;
        };
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from);
        let service_runs = home.as_deref().and_then(service_program);
        let bundled_for_sum = bundled.clone();
        let bundled_build = tokio::task::spawn_blocking(move || {
            crate::fleet::sha256_file(&bundled_for_sum)
                .ok()
                .map(|sum| sum[..16.min(sum.len())].to_owned())
        })
        .await
        .ok()
        .flatten();
        let dev_tree = crate::fleet::from_dev_tree();
        let deadline = tokio::time::Instant::now() + LOCAL_MAXD_WAIT;
        loop {
            let machine = self
                .view
                .read(|merge| machines(merge, None))
                .into_iter()
                .find(|machine| machine.local);
            let step = local_maxd(LocalFacts {
                dev_tree,
                service_runs: service_runs.as_deref(),
                bundled: &bundled,
                bundled_build: bundled_build.as_deref(),
                machine: machine.as_ref(),
            });
            match step {
                LocalMaxd::Leave(why) => {
                    tracing::debug!(why, "left this Mac's maxd as it is");
                    return;
                }
                LocalMaxd::Restart => {
                    tracing::info!(
                        running = ?machine.and_then(|machine| machine.build),
                        bundled = ?bundled_build,
                        "restarting this Mac's maxd onto the build beside the app"
                    );
                    if let Err(error) = self.devices.restart_maxd(local_link()).await {
                        tracing::warn!(%error, "couldn't restart this Mac's maxd onto the new build");
                    }
                    return;
                }
                LocalMaxd::Wait if tokio::time::Instant::now() >= deadline => {
                    tracing::info!("this Mac's maxd didn't say hello, so it stays as it is");
                    return;
                }
                LocalMaxd::Wait => tokio::time::sleep(LOCAL_MAXD_POLL).await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::ids::DeviceId;

    use super::*;

    #[test]
    fn only_a_release_in_an_applications_folder_offers_updates() {
        let release = Path::new("/Applications/MaxHQ.app/Contents/MacOS/maxhq-desktop");
        assert_eq!(offered(release, false), Ok(()));
        assert_eq!(
            offered(
                Path::new("/Users/someone/Applications/MaxHQ.app/Contents/MacOS/maxhq-desktop"),
                false
            ),
            Ok(())
        );
        assert_eq!(offered(release, true), Err(NoUpdates::FromSource));
        assert_eq!(
            offered(
                Path::new("/Users/someone/src/maxhq/target/release/maxhq-desktop"),
                false
            ),
            Err(NoUpdates::FromSource)
        );
        assert_eq!(
            offered(
                Path::new("/Users/someone/src/maxhq/target/debug/MaxHQ Dev.app/Contents/MacOS/maxhq-desktop"),
                false
            ),
            Err(NoUpdates::FromSource)
        );
        assert_eq!(
            offered(
                Path::new("/Volumes/MaxHQ/MaxHQ.app/Contents/MacOS/maxhq-desktop"),
                false
            ),
            Err(NoUpdates::DiskImage)
        );
        assert_eq!(
            offered(
                Path::new("/private/var/folders/x/AppTranslocation/0A1B/d/MaxHQ.app/Contents/MacOS/maxhq-desktop"),
                false
            ),
            Err(NoUpdates::Translocated)
        );
        assert_eq!(
            offered(
                Path::new("/Users/someone/Downloads/MaxHQ.app/Contents/MacOS/maxhq-desktop"),
                false
            ),
            Err(NoUpdates::NotInApplications)
        );
    }

    #[test]
    fn updates_are_looked_for_by_default_and_the_switch_lasts() {
        let registry = Registry::in_memory().unwrap();
        assert_eq!(
            UpdatePrefs::load(&registry),
            UpdatePrefs { auto_check: true }
        );
        let off = UpdatePrefs { auto_check: false };
        off.save(&registry).unwrap();
        assert_eq!(UpdatePrefs::load(&registry), off);
        // Settings saved before a field existed read with its default.
        registry.set_setting(PREFS, &serde_json::json!({})).unwrap();
        assert_eq!(UpdatePrefs::load(&registry), UpdatePrefs::default());
    }

    fn mac(build: Option<&str>, connected: bool, winding_down: bool) -> Machine {
        Machine {
            device: DeviceId::generate(),
            name: "This Mac".into(),
            connected,
            winding_down,
            capabilities: None,
            build: build.map(str::to_owned),
            target: Some("aarch64-apple-darwin".into()),
            local: true,
            workspaces: false,
            tools: Vec::new(),
        }
    }

    const BUNDLED: &str = "/Applications/MaxHQ.app/Contents/MacOS/maxd";
    const NEW: &str = "fedcba9876543210";

    /// A released app whose LaunchAgent runs its own `maxd`, with this Mac
    /// as `machine`.
    fn released(machine: Option<&Machine>) -> LocalFacts<'_> {
        LocalFacts {
            dev_tree: false,
            service_runs: Some(Path::new(BUNDLED)),
            bundled: Path::new(BUNDLED),
            bundled_build: Some(NEW),
            machine,
        }
    }

    #[test]
    fn this_macs_maxd_restarts_only_when_its_service_runs_the_bundled_one_on_another_build() {
        let old = mac(Some("0123456789abcdef"), true, false);
        assert_eq!(local_maxd(released(Some(&old))), LocalMaxd::Restart);
        assert_eq!(
            local_maxd(released(Some(&mac(Some(NEW), true, false)))),
            LocalMaxd::Leave("it runs the bundled build")
        );
        // A maxd from before builds can't say which it runs, so it's behind.
        assert_eq!(
            local_maxd(released(Some(&mac(None, true, false)))),
            LocalMaxd::Restart
        );

        // Before its hello, or while it winds down, the app waits.
        assert_eq!(local_maxd(released(None)), LocalMaxd::Wait);
        assert_eq!(
            local_maxd(released(Some(&mac(Some("0123456789abcdef"), false, false)))),
            LocalMaxd::Wait
        );
        assert_eq!(
            local_maxd(released(Some(&mac(Some("0123456789abcdef"), true, true)))),
            LocalMaxd::Wait
        );

        // The source tree restarts it with Start maxd.
        assert_eq!(
            local_maxd(LocalFacts {
                dev_tree: true,
                ..released(Some(&old))
            }),
            LocalMaxd::Leave("the app runs from the source tree")
        );
        // A service that runs another maxd, or none, is left be.
        assert_eq!(
            local_maxd(LocalFacts {
                service_runs: Some(Path::new("/Users/someone/.cargo/bin/maxd")),
                ..released(Some(&old))
            }),
            LocalMaxd::Leave("the LaunchAgent runs another maxd")
        );
        assert_eq!(
            local_maxd(LocalFacts {
                service_runs: None,
                ..released(Some(&old))
            }),
            LocalMaxd::Leave("no LaunchAgent runs maxd")
        );
        assert_eq!(
            local_maxd(LocalFacts {
                bundled_build: None,
                ..released(Some(&old))
            }),
            LocalMaxd::Leave("the bundled maxd couldn't be read")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_service_that_runs_maxd_through_a_link_runs_the_bundled_one() {
        let dir = std::env::temp_dir().join(format!(
            "maxhq-app-update-{}",
            maxhq_protocol::ids::LinkId::generate()
        ));
        let macos = dir.join("MaxHQ.app/Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::create_dir_all(dir.join(".local/bin")).unwrap();
        let bundled = macos.join("maxd");
        std::fs::write(&bundled, b"maxd").unwrap();
        let link = dir.join(".local/bin/maxd");
        std::os::unix::fs::symlink(&bundled, &link).unwrap();

        // The LaunchAgent file names the link, as `maxd service install`
        // writes it when one points at the program.
        let agents = dir.join("Library/LaunchAgents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("com.maxagenthq.maxd.plist"),
            format!(
                "<plist><dict><key>ProgramArguments</key><array><string>{}</string><string>run</string></array></dict></plist>",
                link.display()
            ),
        )
        .unwrap();
        let program = service_program(&dir).unwrap();
        assert_eq!(program, link);
        let old = mac(Some("0123456789abcdef"), true, false);
        assert_eq!(
            local_maxd(LocalFacts {
                dev_tree: false,
                service_runs: Some(&program),
                bundled: &bundled,
                bundled_build: Some("fedcba9876543210"),
                machine: Some(&old),
            }),
            LocalMaxd::Restart
        );
        assert_eq!(service_program(&dir.join("nobody")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
