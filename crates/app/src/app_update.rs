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
//!
//! On Linux the AppImage is what updates, the way it does on macOS: the
//! updater writes the new one over it. A deb or rpm belongs to its package
//! manager, so that copy points you to the new package instead. The
//! service there runs a copy of the bundled `maxd` through
//! `~/.local/bin/maxd`, so moving it onto a new build copies the new one
//! out and flips the link before the restart.
//!
//! On Windows the installer that put the app in place runs again for the
//! update: the NSIS setup for one user, or the MSI for everyone on the PC.
//! Each copy looks for the update built for its own installer, since the
//! other would install a second app beside it. The service there runs a
//! copy of the bundled `maxd` too, through a junction, because Windows
//! won't let an installer replace a program that runs.

use std::path::{Path, PathBuf};
use std::time::Duration;

use maxhq_protocol::time::Timestamp;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::devices::Connector;
use crate::fleet::{machines, Machine, DEV_BUNDLE};
use crate::registry::{Registry, RegistryError};
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
    /// It was installed from a deb or rpm package, which its package
    /// manager updates.
    Package,
    /// It's an AppImage in a folder the app can't write to, so an update
    /// can't replace it.
    Unwritable,
}

/// Whether the app whose program is at `exe` offers updates. `debug` says
/// it's a debug build.
pub fn offered(exe: &Path, debug: bool) -> Result<(), NoUpdates> {
    if cfg!(target_os = "linux") {
        offered_on_linux(&LinuxApp::here(exe), debug, takes_files)
    } else if cfg!(windows) {
        offered_on_windows(WindowsApp::here(exe), debug)
    } else {
        offered_on_macos(exe, debug)
    }
}

/// [`offered`] on macOS, where only a bundle in an Applications folder
/// updates.
fn offered_on_macos(exe: &Path, debug: bool) -> Result<(), NoUpdates> {
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

/// How the app was installed on Linux.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinuxApp {
    /// It runs from the AppImage at this path.
    AppImage(PathBuf),
    /// A deb or rpm package put it under `/usr`.
    Package,
    /// Neither: a build from the source tree, or one copied somewhere by
    /// hand.
    Loose,
}

impl LinuxApp {
    /// How the app whose program is at `exe` was installed, given the
    /// `APPIMAGE` and `APPDIR` an AppImage's runtime sets. The program has
    /// to be inside `APPDIR`, since a program the AppImage started gets
    /// both as well.
    pub fn of(exe: &Path, appimage: Option<&Path>, appdir: Option<&Path>) -> Self {
        if let (Some(file), Some(dir)) = (appimage, appdir)
            && exe.starts_with(dir)
        {
            return Self::AppImage(file.to_owned());
        }
        // `/usr/local` is for what you build and install yourself.
        if exe.starts_with("/usr") && !exe.starts_with("/usr/local") {
            Self::Package
        } else {
            Self::Loose
        }
    }

    /// How this app, whose program is at `exe`, was installed.
    pub fn here(exe: &Path) -> Self {
        let var = |name| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Self::of(exe, var("APPIMAGE").as_deref(), var("APPDIR").as_deref())
    }

    /// Whether it was installed, rather than run from where it was built.
    pub fn installed(&self) -> bool {
        !matches!(self, Self::Loose)
    }
}

/// [`offered`] on Linux. `writable` says whether the app can make files in
/// a folder, which the updater needs to swap the AppImage in it.
fn offered_on_linux(
    app: &LinuxApp,
    debug: bool,
    writable: impl FnOnce(&Path) -> bool,
) -> Result<(), NoUpdates> {
    if debug {
        return Err(NoUpdates::FromSource);
    }
    match app {
        LinuxApp::AppImage(file) => match file.parent() {
            Some(dir) if writable(dir) => Ok(()),
            _ => Err(NoUpdates::Unwritable),
        },
        LinuxApp::Package => Err(NoUpdates::Package),
        LinuxApp::Loose => Err(NoUpdates::FromSource),
    }
}

/// How the app was installed on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsApp {
    /// The NSIS setup installed it for you, and left its uninstaller beside
    /// the program.
    Nsis,
    /// The MSI installed it for everyone on the PC, under Program Files.
    Msi,
    /// Neither: a build from the source tree, or one copied somewhere by
    /// hand.
    Loose,
}

impl WindowsApp {
    /// The name the NSIS setup gives its uninstaller, beside the program.
    pub const UNINSTALLER: &'static str = "uninstall.exe";

    /// How the app whose program is at `exe` was installed. `uninstaller`
    /// says whether the NSIS setup's uninstaller is beside it, and
    /// `program_files` are the folders an MSI installs into.
    pub fn of(exe: &Path, uninstaller: bool, program_files: &[PathBuf]) -> Self {
        if uninstaller {
            Self::Nsis
        } else if program_files.iter().any(|dir| windows_inside(exe, dir)) {
            Self::Msi
        } else {
            Self::Loose
        }
    }

    /// How this app, whose program is at `exe`, was installed.
    pub fn here(exe: &Path) -> Self {
        let uninstaller = exe.with_file_name(Self::UNINSTALLER).is_file();
        let program_files: Vec<PathBuf> = ["ProgramFiles", "ProgramW6432"]
            .into_iter()
            .filter_map(std::env::var_os)
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .collect();
        Self::of(exe, uninstaller, &program_files)
    }

    /// Whether it was installed, rather than run from where it was built.
    pub fn installed(self) -> bool {
        self != Self::Loose
    }
}

/// Whether `path` is inside `dir`, by Windows' rules: either slash parts
/// folders, case doesn't matter and the `\\?\` prefix changes nothing.
/// Plain text, so it reads the same on any system.
pub fn windows_inside(path: &Path, dir: &Path) -> bool {
    let fold = |path: &Path| {
        let text = path.to_string_lossy().replace('/', "\\").to_lowercase();
        match text.strip_prefix(r"\\?\") {
            Some(rest) => rest.to_owned(),
            None => text,
        }
    };
    let dir = fold(dir);
    let dir = dir.trim_end_matches('\\');
    !dir.is_empty()
        && fold(path)
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('\\'))
}

/// [`offered`] on Windows, where either installer updates the copy it put
/// in place.
fn offered_on_windows(app: WindowsApp, debug: bool) -> Result<(), NoUpdates> {
    if debug || !app.installed() {
        return Err(NoUpdates::FromSource);
    }
    Ok(())
}

/// Whether the app can make files in `dir`, tried with a folder it makes
/// and takes away again, since only trying tells for sure.
fn takes_files(dir: &Path) -> bool {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let probe = dir.join(format!(".maxhq-write-check-{}-{nonce}", std::process::id()));
    match std::fs::create_dir(&probe) {
        Ok(()) => {
            let _ = std::fs::remove_dir(&probe);
            true
        }
        Err(_) => false,
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
    /// The program the service runs, if there is one: the LaunchAgent on
    /// macOS and the systemd user unit on Linux.
    pub service_runs: Option<&'a Path>,
    /// The program the service runs when the app installed `maxd`: the
    /// `maxd` beside the app on macOS, and the link to its copy elsewhere.
    pub bundled: &'a Path,
    /// The build of the `maxd` beside the app, if the app could read it.
    pub bundled_build: Option<&'a str>,
    /// This Mac, as the view has it.
    pub machine: Option<&'a Machine>,
}

/// Whether to restart this Mac's `maxd` onto the build beside the app. Only
/// when its service runs that very program, so an app that isn't the
/// one installing `maxd` leaves it be, and only once its `maxd` says which
/// build it runs.
pub fn local_maxd(facts: LocalFacts<'_>) -> LocalMaxd {
    if facts.dev_tree {
        return LocalMaxd::Leave("the app runs from the source tree");
    }
    let Some(program) = facts.service_runs else {
        return LocalMaxd::Leave("no service runs maxd");
    };
    if !same_file(program, facts.bundled) {
        return LocalMaxd::Leave("the service runs another maxd");
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

/// The program the service in `home` runs, if there's one: the systemd
/// user unit's on Linux, the logon task's on Windows, which MaxHQ's folder
/// holds whatever `home` is, and the LaunchAgent's elsewhere.
pub fn service_program(home: &Path) -> Option<PathBuf> {
    if cfg!(target_os = "linux") {
        let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        unit_program(&config_home(home, xdg.as_deref()))
    } else if cfg!(windows) {
        maxhq_core::service::task_program_here()
    } else {
        launch_agent_program(home)
    }
}

/// The program the LaunchAgent in `home` runs, if there's one.
fn launch_agent_program(home: &Path) -> Option<PathBuf> {
    let file = maxhq_core::service::launch_agent_file(home);
    let plist = std::fs::read_to_string(file).ok()?;
    maxhq_core::service::program_in(&plist)
}

/// The program the systemd user unit under `config_home` runs, if there's
/// one.
fn unit_program(config_home: &Path) -> Option<PathBuf> {
    let unit = std::fs::read_to_string(maxhq_core::service::unit_file(config_home)).ok()?;
    maxhq_core::service::program_in_unit(&unit)
}

/// Where systemd looks for user units' folder: `XDG_CONFIG_HOME` when it's
/// an absolute path, as the spec wants, and `~/.config` otherwise. `maxd
/// service install` writes its unit by the same rule.
fn config_home(home: &Path, xdg: Option<&Path>) -> PathBuf {
    match xdg {
        Some(dir) if dir.is_absolute() => dir.to_owned(),
        _ => home.join(".config"),
    }
}

/// Your home folder: `HOME` on Unix, if it's set, and your profile
/// folder on Windows.
pub(crate) fn home() -> Option<PathBuf> {
    maxhq_os::home::home_dir()
}

impl<C: Connector> App<C> {
    /// Brings this Mac's `maxd` onto the build beside the app, once it says
    /// hello, when its service runs that `maxd` and it runs another
    /// build: after an update, or after you put a new copy of the app in
    /// place by hand. It restarts the service the way Restart maxd does,
    /// and agents keep running across that. It decides once each start.
    pub async fn keep_local_maxd_current(&self) {
        let Some(bundled) = crate::devices::bundled_maxd() else {
            return;
        };
        let home = home();
        let service_runs = home.as_deref().and_then(service_program);
        // Off macOS the service runs a copy of the bundled maxd, through a
        // link that keeps its name across builds.
        let installed = self
            .devices
            .local_maxd_program(home.as_deref())
            .unwrap_or_else(|| bundled.clone());
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
                bundled: &installed,
                bundled_build: bundled_build.as_deref(),
                machine: machine.as_ref(),
            });
            match step {
                LocalMaxd::Leave(why) => {
                    tracing::debug!(why, "left the local maxd as it is");
                    return;
                }
                LocalMaxd::Restart => {
                    tracing::info!(
                        running = ?machine.and_then(|machine| machine.build),
                        bundled = ?bundled_build,
                        "restarting the local maxd onto the build beside the app"
                    );
                    if let Err(error) = self.devices.update_local_maxd(home.clone()).await {
                        tracing::warn!(%error, "couldn't restart the local maxd onto the new build");
                    }
                    return;
                }
                LocalMaxd::Wait if tokio::time::Instant::now() >= deadline => {
                    tracing::info!("the local maxd didn't say hello, so it stays as it is");
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
        assert_eq!(offered_on_macos(release, false), Ok(()));
        assert_eq!(
            offered_on_macos(
                Path::new("/Users/someone/Applications/MaxHQ.app/Contents/MacOS/maxhq-desktop"),
                false
            ),
            Ok(())
        );
        assert_eq!(offered_on_macos(release, true), Err(NoUpdates::FromSource));
        assert_eq!(
            offered_on_macos(
                Path::new("/Users/someone/src/maxhq/target/release/maxhq-desktop"),
                false
            ),
            Err(NoUpdates::FromSource)
        );
        assert_eq!(
            offered_on_macos(
                Path::new("/Users/someone/src/maxhq/target/debug/MaxHQ Dev.app/Contents/MacOS/maxhq-desktop"),
                false
            ),
            Err(NoUpdates::FromSource)
        );
        assert_eq!(
            offered_on_macos(
                Path::new("/Volumes/MaxHQ/MaxHQ.app/Contents/MacOS/maxhq-desktop"),
                false
            ),
            Err(NoUpdates::DiskImage)
        );
        assert_eq!(
            offered_on_macos(
                Path::new("/private/var/folders/x/AppTranslocation/0A1B/d/MaxHQ.app/Contents/MacOS/maxhq-desktop"),
                false
            ),
            Err(NoUpdates::Translocated)
        );
        assert_eq!(
            offered_on_macos(
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
            version: None,
            target: Some("aarch64-apple-darwin".into()),
            local: true,
            workspaces: false,
            tools: Vec::new(),
        }
    }

    const BUNDLED: &str = "/Applications/MaxHQ.app/Contents/MacOS/maxd";
    const NEW: &str = "fedcba9876543210";

    /// A released app whose service runs its own `maxd`, with this Mac
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
            LocalMaxd::Leave("the service runs another maxd")
        );
        assert_eq!(
            local_maxd(LocalFacts {
                service_runs: None,
                ..released(Some(&old))
            }),
            LocalMaxd::Leave("no service runs maxd")
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
        let program = launch_agent_program(&dir).unwrap();
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
        assert_eq!(launch_agent_program(&dir.join("nobody")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn on_linux_an_appimage_updates_and_a_package_points_to_its_package() {
        let appdir = Path::new("/tmp/.mount_MaxHQ1a2b");
        let exe = appdir.join("usr/bin/maxhq-desktop");
        let file = Path::new("/home/someone/Applications/MaxHQ.AppImage");
        let appimage = LinuxApp::of(&exe, Some(file), Some(appdir));
        assert_eq!(appimage, LinuxApp::AppImage(file.to_owned()));
        assert!(appimage.installed());

        // A program an AppImage started gets its variables too, but it
        // isn't the AppImage.
        let package = Path::new("/usr/bin/maxhq-desktop");
        assert_eq!(
            LinuxApp::of(package, Some(file), Some(appdir)),
            LinuxApp::Package
        );
        assert_eq!(
            LinuxApp::of(&exe, Some(file), None),
            LinuxApp::Loose,
            "half an AppImage's variables are none"
        );
        assert_eq!(LinuxApp::of(package, None, None), LinuxApp::Package);
        let built = [
            "/usr/local/bin/maxhq-desktop",
            "/home/someone/src/maxhq/target/release/maxhq-desktop",
            "/usrlocal/maxhq-desktop",
        ];
        for exe in built {
            assert_eq!(
                LinuxApp::of(Path::new(exe), None, None),
                LinuxApp::Loose,
                "{exe}"
            );
        }
        assert!(!LinuxApp::Loose.installed());

        let asked = std::cell::RefCell::new(Vec::new());
        let writable = |answer: bool| {
            let asked = &asked;
            move |dir: &Path| {
                asked.borrow_mut().push(dir.to_owned());
                answer
            }
        };
        assert_eq!(offered_on_linux(&appimage, false, writable(true)), Ok(()));
        assert_eq!(
            asked.take(),
            [Path::new("/home/someone/Applications")],
            "the updater swaps the AppImage in its folder"
        );
        assert_eq!(
            offered_on_linux(&appimage, false, writable(false)),
            Err(NoUpdates::Unwritable)
        );
        assert_eq!(
            offered_on_linux(&appimage, true, writable(true)),
            Err(NoUpdates::FromSource)
        );
        assert_eq!(
            offered_on_linux(&LinuxApp::Package, false, writable(true)),
            Err(NoUpdates::Package)
        );
        assert_eq!(
            offered_on_linux(&LinuxApp::Loose, false, writable(true)),
            Err(NoUpdates::FromSource)
        );
    }

    #[test]
    fn on_windows_each_installer_updates_its_own_copy_and_a_build_does_not() {
        let program_files = [
            PathBuf::from(r"C:\Program Files"),
            PathBuf::from(r"C:\Program Files\"),
        ];
        let nsis = Path::new(r"C:\Users\<user>\AppData\Local\MaxHQ\MaxHQ.exe");
        let msi = Path::new(r"C:\Program Files\MaxHQ\MaxHQ.exe");
        let built = Path::new(r"C:\src\maxhq\target\release\maxhq-desktop.exe");
        assert_eq!(WindowsApp::of(nsis, true, &program_files), WindowsApp::Nsis);
        assert_eq!(WindowsApp::of(msi, false, &program_files), WindowsApp::Msi);
        // The NSIS setup's uninstaller tells, wherever it installed.
        assert_eq!(WindowsApp::of(msi, true, &program_files), WindowsApp::Nsis);
        // Case and slashes don't matter, but a folder named alike does.
        let lower = Path::new("c:/program files/MaxHQ/MaxHQ.exe");
        assert_eq!(
            WindowsApp::of(lower, false, &program_files),
            WindowsApp::Msi
        );
        let verbatim = Path::new(r"\\?\C:\Program Files\MaxHQ\MaxHQ.exe");
        assert_eq!(
            WindowsApp::of(verbatim, false, &program_files),
            WindowsApp::Msi
        );
        assert!(!windows_inside(
            Path::new(r"C:\Program Files"),
            &program_files[0]
        ));
        let alike = Path::new(r"C:\Program Files Extra\MaxHQ\MaxHQ.exe");
        assert_eq!(
            WindowsApp::of(alike, false, &program_files),
            WindowsApp::Loose
        );
        assert_eq!(
            WindowsApp::of(built, false, &program_files),
            WindowsApp::Loose
        );
        assert_eq!(WindowsApp::of(msi, false, &[]), WindowsApp::Loose);
        assert_eq!(
            WindowsApp::of(msi, false, &[PathBuf::new()]),
            WindowsApp::Loose,
            "an empty folder holds nothing"
        );

        assert_eq!(offered_on_windows(WindowsApp::Nsis, false), Ok(()));
        assert_eq!(offered_on_windows(WindowsApp::Msi, false), Ok(()));
        assert_eq!(
            offered_on_windows(WindowsApp::Nsis, true),
            Err(NoUpdates::FromSource)
        );
        assert_eq!(
            offered_on_windows(WindowsApp::Loose, false),
            Err(NoUpdates::FromSource)
        );
        assert!(WindowsApp::Msi.installed());
        assert!(!WindowsApp::Loose.installed());
    }

    #[test]
    fn the_nsis_uninstaller_beside_the_program_marks_its_install() {
        let dir = std::env::temp_dir().join(format!("maxhq-nsis-{}", DeviceId::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("MaxHQ.exe");
        assert_eq!(WindowsApp::here(&exe), WindowsApp::Loose);
        std::fs::write(dir.join(WindowsApp::UNINSTALLER), b"").unwrap();
        assert_eq!(WindowsApp::here(&exe), WindowsApp::Nsis);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_write_check_tells_a_folder_it_can_write_to_and_leaves_nothing() {
        let dir = std::env::temp_dir().join(format!(
            "maxhq-write-check-{}",
            maxhq_protocol::ids::LinkId::generate()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(takes_files(&dir));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        assert!(!takes_files(&dir.join("missing")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // systemd is Linux only, and its quoting reads the `\` in a Windows
    // path as an escape.
    #[cfg(unix)]
    #[test]
    fn the_systemd_unit_says_which_maxd_the_service_runs() {
        let dir = std::env::temp_dir().join(format!(
            "maxhq-app-update-{}",
            maxhq_protocol::ids::LinkId::generate()
        ));
        let config = dir.join(".config");
        assert_eq!(config_home(&dir, None), config);
        assert_eq!(
            config_home(&dir, Some(Path::new("relative/config"))),
            config,
            "systemd skips a relative XDG_CONFIG_HOME"
        );
        assert_eq!(config_home(&dir, Some(&dir.join("xdg"))), dir.join("xdg"));

        assert_eq!(unit_program(&config), None);
        let units = config.join("systemd/user");
        std::fs::create_dir_all(&units).unwrap();
        let link = dir.join(".local/bin/maxd");
        std::fs::write(
            units.join("maxd.service"),
            format!(
                "[Unit]\nDescription=maxd\n\n[Service]\nExecStart=\"{}\" run\nRestart=on-failure\n",
                link.display()
            ),
        )
        .unwrap();
        let program = unit_program(&config).unwrap();
        assert_eq!(program, link);
        let old = mac(Some("0123456789abcdef"), true, false);
        assert_eq!(
            local_maxd(LocalFacts {
                dev_tree: false,
                service_runs: Some(&program),
                bundled: &link,
                bundled_build: Some(NEW),
                machine: Some(&old),
            }),
            LocalMaxd::Restart,
            "the unit runs the app's link, on another build"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
