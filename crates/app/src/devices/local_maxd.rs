// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! This machine's own `maxd`, which ships with the app. The app installs
//! it as a login service with `maxd service install` and takes it out
//! again with `maxd service uninstall`, then probes its link so the change
//! shows at once. When you also ask for MaxHQ's data to go, the app deletes
//! its own folders and key once nothing it runs would write there again.
//! Where the service's `maxd` lives differs per system, and `os` holds
//! that: macOS runs the bundled one, other systems a copy of it, which
//! Windows keeps in its own folders.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{failed, lock, Connector, Devices};
use crate::registry::{local_link, Registry};
use crate::request::CommandError;

#[cfg(target_os = "macos")]
mod macos;
// Its copy-out is tested on the Mac too.
#[cfg(any(not(target_os = "macos"), test))]
mod other;
// Its layout and its misplaced check are tested anywhere.
#[cfg(any(windows, test))]
mod windows;

#[cfg(target_os = "macos")]
use macos as os;
#[cfg(not(any(target_os = "macos", windows)))]
use other as os;
#[cfg(windows)]
use windows as os;

impl<C: Connector> Devices<C> {
    /// Installs this machine's `maxd` as a login service with
    /// `maxd service install`, then probes its link so the app connects
    /// without waiting for the next retry. Refused while the app runs from
    /// a disk image or a copy macOS moved aside, since the service would
    /// point at a `maxd` that goes away. Off macOS the bundled `maxd` is
    /// first copied out to `~/.local/lib/maxhq/versions`, and the service
    /// runs that copy through `~/.local/bin/maxd`. On Windows the copy
    /// goes to `%LOCALAPPDATA%\MaxHQ\versions`, and the service runs it
    /// through the junction `bin\current` beside them.
    pub async fn start_local_maxd(&self) -> Result<(), CommandError> {
        self.start_local_maxd_in(crate::app_update::home()).await
    }

    /// [`Self::start_local_maxd`], with the home folder the copy goes in.
    async fn start_local_maxd_in(&self, home: Option<PathBuf>) -> Result<(), CommandError> {
        let maxd = self.bundled_maxd()?;
        if let Some(reason) = os::misplaced(&maxd) {
            return Err(failed(reason.to_owned()));
        }
        let program = self.place_maxd(maxd, home).await?;
        self.run_maxd(program, vec!["service", "install"]).await?;
        self.refresh_links(Some(local_link()))
    }

    /// The program this machine's service runs once the app installed
    /// `maxd`, given the home folder: the bundled `maxd` on macOS,
    /// `bin\current\maxd.exe` in MaxHQ's folder on Windows and
    /// `~/.local/bin/maxd` elsewhere. `None` when this build has no `maxd`
    /// or there's no folder to copy one to.
    pub(crate) fn local_maxd_program(&self, home: Option<&Path>) -> Option<PathBuf> {
        os::installed(self.maxd.as_deref()?, home)
    }

    /// Moves this machine's service onto the bundled `maxd` and restarts
    /// it. Off macOS that copies the new build out and points the link at
    /// it first.
    pub(crate) async fn update_local_maxd(
        &self,
        home: Option<PathBuf>,
    ) -> Result<(), CommandError> {
        let maxd = self.bundled_maxd()?;
        self.place_maxd(maxd, home).await?;
        self.restart_maxd(local_link()).await
    }

    /// Stops this machine's `maxd` and every agent it runs, and takes it
    /// out of the login items, with `maxd service uninstall`, then probes
    /// its link so the app shows it gone. With `delete_data` the `maxd`
    /// also deletes its own data. The app's data is
    /// [`Self::erase_app_data`]'s.
    pub async fn uninstall_local_maxd(&self, delete_data: bool) -> Result<(), CommandError> {
        let maxd = self.bundled_maxd()?;
        let mut args = vec!["service", "uninstall"];
        if delete_data {
            args.push("--delete-data");
        }
        self.run_maxd(maxd, args).await?;
        self.refresh_links(Some(local_link()))
    }

    /// Deletes the app's own data for good: the `folders` the shell names,
    /// this Mac's app key and the `Include` line in `~/.ssh/config`. First
    /// it stops all that would write there again: MaxHQ's SSH files stop,
    /// the device list moves into memory, which closes the database, and
    /// every link closes. It goes as far as it can and then says what
    /// failed. Either way the app has no devices left, so it should quit.
    pub async fn erase_app_data(&self, folders: Vec<PathBuf>) -> Result<(), CommandError> {
        let mut failures = Vec::new();
        if let Some(files) = self.core.ssh.get() {
            files.stop();
            if let Err(error) = files.remove_include() {
                failures.push(format!(
                    "Couldn't take MaxHQ's line out of ~/.ssh/config: {error}"
                ));
            }
        }
        match Registry::in_memory() {
            // The old list closes its database as it drops.
            Ok(memory) => drop(std::mem::replace(&mut *lock(&self.core.registry), memory)),
            Err(error) => failures.push(format!("Couldn't close the device list: {error}")),
        }
        // Dropping a link closes it, so no report or picture comes in.
        drop(std::mem::take(&mut *lock(&self.core.links)));
        if let Some(keys) = self.core.app_key.get()
            && let Err(error) = keys.forget()
        {
            failures.push(error);
        }
        let removed = self
            .core
            .runtime
            .spawn_blocking(move || remove_folders(&folders))
            .await
            .map_err(|error| format!("Deleting MaxHQ's data stopped: {error}"))
            .and_then(|removed| removed);
        if let Err(error) = removed {
            failures.push(error);
        }
        if failures.is_empty() {
            tracing::info!("deleted the app's data");
            return Ok(());
        }
        tracing::error!(?failures, "couldn't delete all of the app's data");
        Err(failed(failures.join(". ")))
    }

    /// The `maxd` in the app's bundle.
    fn bundled_maxd(&self) -> Result<PathBuf, CommandError> {
        self.maxd
            .clone()
            .ok_or_else(|| failed("This build can't find its maxd".to_owned()))
    }

    /// Puts the bundled `maxd` where the service runs it, on a blocking
    /// thread, and gives the program to run.
    async fn place_maxd(
        &self,
        maxd: PathBuf,
        home: Option<PathBuf>,
    ) -> Result<PathBuf, CommandError> {
        present(&maxd)?;
        self.core
            .runtime
            .spawn_blocking(move || os::stage(&maxd, home.as_deref()))
            .await
            .map_err(|error| failed(format!("Copying maxd stopped: {error}")))?
            .map_err(|error| failed(format!("Couldn't copy maxd out of the app: {error}")))
    }

    /// Runs `maxd` with `args` on a blocking thread, and waits for it.
    async fn run_maxd(&self, maxd: PathBuf, args: Vec<&'static str>) -> Result<(), CommandError> {
        self.core
            .runtime
            .spawn_blocking(move || run(&maxd, &args))
            .await
            .map_err(|error| failed(format!("Running maxd stopped: {error}")))?
    }
}

/// Fails unless there's a `maxd` at `maxd`.
fn present(maxd: &Path) -> Result<(), CommandError> {
    if maxd.is_file() {
        return Ok(());
    }
    Err(failed(format!("There's no maxd at {}", maxd.display())))
}

/// Runs `maxd` with `args`. A failure gives what `maxd` wrote to stderr.
fn run(maxd: &Path, args: &[&str]) -> Result<(), CommandError> {
    present(maxd)?;
    let command = format!("maxd {}", args.join(" "));
    let output = Command::new(maxd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| failed(format!("Couldn't run maxd: {error}")))?;
    if output.status.success() {
        let report = String::from_utf8_lossy(&output.stdout);
        tracing::info!(%command, %report, "maxd ran its service command");
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    tracing::warn!(%command, status = %output.status, %stderr, "maxd's service command failed");
    Err(failed(match stderr.trim() {
        "" => format!("{command} failed with {}", output.status),
        reason => reason.to_owned(),
    }))
}

/// Removes each folder and all in it. One that's gone already is fine.
fn remove_folders(folders: &[PathBuf]) -> Result<(), String> {
    for folder in folders {
        match std::fs::remove_dir_all(folder) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                return Err(format!("Couldn't delete {}: {error}", folder.display()));
            }
            _ => tracing::info!(folder = %folder.display(), "deleted"),
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    use maxhq_protocol::ids::LinkId;
    use zeroize::Zeroizing;

    use super::*;
    use crate::app_keys::AppKeys;
    use crate::devices::tests::{harness, Call};
    use crate::keys::{Key, KeyStore, MemoryStore};
    use crate::tests::{block_on, FakeShell};
    use crate::{App, Caller, Request, Response};

    /// A folder under the system's temp folder, with a fake `maxd` that
    /// writes its arguments to `args` beside it. Removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            Self::under("")
        }

        /// One with its `maxd` at `inner` below the folder.
        fn under(inner: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("maxhq-maxd-{}", LinkId::generate()));
            std::fs::create_dir_all(dir.join(inner)).unwrap();
            let scratch = Self(dir);
            scratch.script(inner, r#"printf '%s ' "$@" > "$0.args""#);
            scratch
        }

        /// Makes the `maxd` at `inner` run `body`.
        fn script(&self, inner: &str, body: &str) {
            let maxd = self.0.join(inner).join("maxd");
            std::fs::write(&maxd, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&maxd, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        fn maxd(&self, inner: &str) -> PathBuf {
            self.0.join(inner).join("maxd")
        }

        /// What the `maxd` at `inner` last ran with, taken.
        fn args(&self, inner: &str) -> Option<String> {
            let file = self.0.join(inner).join("maxd.args");
            let args = std::fs::read_to_string(&file).ok()?;
            std::fs::remove_file(file).unwrap();
            Some(args.trim().to_owned())
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A key store the test still reads once the keys hold it.
    #[derive(Clone, Default)]
    struct Shared(Arc<MemoryStore>);

    impl KeyStore for Shared {
        fn load(&self, key: Key) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
            self.0.load(key)
        }

        fn save(&self, key: Key, bytes: &[u8]) -> Result<(), String> {
            self.0.save(key, bytes)
        }

        fn forget(&self, key: Key) -> Result<(), String> {
            self.0.forget(key)
        }
    }

    /// A store whose forget fails, as a locked keychain's does.
    struct Locked;

    impl KeyStore for Locked {
        fn load(&self, _key: Key) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
            Ok(None)
        }

        fn save(&self, _key: Key, _bytes: &[u8]) -> Result<(), String> {
            Ok(())
        }

        fn forget(&self, _key: Key) -> Result<(), String> {
            Err("Couldn't forget this Mac's app key: the keychain is locked".into())
        }
    }

    /// The app's data folder with a device list open in it and MaxHQ's
    /// SSH files, as the shell sets them up, and a cache folder beside it.
    fn app_folders(scratch: &Scratch) -> (PathBuf, PathBuf, Registry) {
        let data = scratch.0.join("data");
        let cache = scratch.0.join("cache");
        std::fs::create_dir_all(data.join("ssh")).unwrap();
        std::fs::create_dir_all(cache.join("media")).unwrap();
        std::fs::write(cache.join("media/picture.png"), b"png").unwrap();
        let registry = Registry::open(&data.join("maxhq.db")).unwrap();
        (data, cache, registry)
    }

    fn caller() -> Caller {
        Caller {
            window: "main".into(),
            on_screen: true,
        }
    }

    /// Where the service's `maxd` runs from, below the scratch folder,
    /// with the scratch folder's `home` as the home folder: the bundled
    /// one on macOS and the copy's link elsewhere.
    const RUNS: &str = if cfg!(target_os = "macos") {
        ""
    } else {
        "home/.local/bin"
    };

    #[test]
    fn starting_the_local_maxd_runs_its_service_install() {
        let scratch = Scratch::new();
        let home = Some(scratch.0.join("home"));
        std::fs::remove_file(scratch.maxd("")).unwrap();
        let h = harness(Registry::in_memory().unwrap(), Some(scratch.maxd("")));
        let start = || block_on(h.devices.start_local_maxd_in(home.clone()));
        assert_eq!(
            start(),
            Err(failed(format!(
                "There's no maxd at {}",
                scratch.maxd("").display()
            )))
        );

        scratch.script("", r#"printf '%s ' "$@" > "$0.args""#);
        start().unwrap();
        assert_eq!(scratch.args(RUNS).as_deref(), Some("service install"));
        assert_eq!(
            h.fake.calls(),
            [(local_link(), Call::Refresh)],
            "the app connects right away"
        );

        scratch.script("", "echo 'launchctl refused the service' >&2\nexit 3");
        assert_eq!(
            start(),
            Err(failed("launchctl refused the service".to_owned()))
        );
        assert!(h.fake.calls().is_empty());

        scratch.script("", "exit 4");
        assert_eq!(
            start(),
            Err(failed(
                "maxd service install failed with exit status: 4".to_owned()
            ))
        );

        let none = harness(Registry::in_memory().unwrap(), None);
        assert_eq!(
            block_on(none.devices.start_local_maxd()),
            Err(failed("This build can't find its maxd".to_owned()))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_copy_macos_moved_aside_or_a_disk_image_doesnt_install_maxd() {
        let inner = "AppTranslocation/0A1B/d/MaxHQ.app/Contents/MacOS";
        let scratch = Scratch::under(inner);
        let h = harness(Registry::in_memory().unwrap(), Some(scratch.maxd(inner)));
        assert_eq!(
            block_on(h.devices.start_local_maxd()),
            Err(failed(os::MISPLACED.to_owned()))
        );
        assert_eq!(scratch.args(inner), None, "maxd never ran");
        assert!(h.fake.calls().is_empty());

        // Uninstalling still works from there.
        block_on(h.devices.uninstall_local_maxd(false)).unwrap();
        assert_eq!(scratch.args(inner).as_deref(), Some("service uninstall"));

        let misplaced = |maxd: &str| os::misplaced(Path::new(maxd)).is_some();
        assert!(misplaced("/Volumes/MaxHQ/MaxHQ.app/Contents/MacOS/maxd"));
        assert!(!misplaced("/Applications/MaxHQ.app/Contents/MacOS/maxd"));
        assert!(!misplaced(
            "/Users/someone/Applications/MaxHQ.app/Contents/MacOS/maxd"
        ));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn an_appimages_maxd_is_installed_from_a_copy_that_outlives_the_mount() {
        let inner = ".mount_MaxHQ1/usr/bin";
        let scratch = Scratch::under(inner);
        let home = scratch.0.join("home");
        let h = harness(Registry::in_memory().unwrap(), Some(scratch.maxd(inner)));
        block_on(h.devices.start_local_maxd_in(Some(home.clone()))).unwrap();
        assert_eq!(scratch.args(RUNS).as_deref(), Some("service install"));
        assert_eq!(scratch.args(inner), None, "the mounted maxd never ran");
        let link = home.join(".local/bin/maxd");
        assert_eq!(
            h.devices.local_maxd_program(Some(&home)),
            Some(link.clone())
        );
        assert_eq!(h.devices.local_maxd_program(None), None);

        // The next run mounts the AppImage elsewhere, and the service
        // still has its maxd.
        std::fs::remove_dir_all(scratch.0.join(".mount_MaxHQ1")).unwrap();
        let status = Command::new(&link).arg("run").status().unwrap();
        assert!(status.success());
        assert_eq!(scratch.args(RUNS).as_deref(), Some("run"));

        let none = harness(Registry::in_memory().unwrap(), None);
        assert_eq!(none.devices.local_maxd_program(Some(&home)), None);
    }

    #[test]
    fn uninstalling_maxd_runs_its_service_uninstall_and_probes_the_link() {
        let scratch = Scratch::new();
        let h = harness(Registry::in_memory().unwrap(), Some(scratch.maxd("")));
        block_on(h.devices.uninstall_local_maxd(false)).unwrap();
        assert_eq!(scratch.args("").as_deref(), Some("service uninstall"));
        assert_eq!(h.fake.calls(), [(local_link(), Call::Refresh)]);

        block_on(h.devices.uninstall_local_maxd(true)).unwrap();
        assert_eq!(
            scratch.args("").as_deref(),
            Some("service uninstall --delete-data")
        );
        assert_eq!(h.fake.calls(), [(local_link(), Call::Refresh)]);

        scratch.script("", "echo 'An agent would not stop' >&2\nexit 1");
        assert_eq!(
            block_on(h.devices.uninstall_local_maxd(false)),
            Err(failed("An agent would not stop".to_owned()))
        );
        assert!(h.fake.calls().is_empty());

        let none = harness(Registry::in_memory().unwrap(), None);
        assert_eq!(
            block_on(none.devices.uninstall_local_maxd(true)),
            Err(failed("This build can't find its maxd".to_owned()))
        );
    }

    #[test]
    fn deleting_the_app_data_leaves_nothing_that_writes_it_back() {
        let scratch = Scratch::new();
        let (data, cache, registry) = app_folders(&scratch);
        let devbox = registry.add("dev@devbox").unwrap();
        let home = scratch.0.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let h = harness(registry, Some(scratch.maxd("")));
        h.devices.keep_ssh(data.join("ssh"), home.clone()).unwrap();
        let config = home.join(".ssh/config");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        let include = format!("Include \"{}/config\"\n", data.join("ssh").display());
        std::fs::write(&config, format!("{include}Host devbox\n")).unwrap();
        let store = Shared::default();
        let keys = AppKeys::open(Box::new(store.clone()), None, "MaxHQ".into()).unwrap();
        h.devices.keep_app_key(keys);
        assert!(store.load(Key::App).unwrap().is_some());

        block_on(h.devices.erase_app_data(vec![data.clone(), cache.clone()])).unwrap();
        assert!(!data.exists());
        assert!(!cache.exists());
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "Host devbox\n");
        assert_eq!(store.load(Key::App).unwrap(), None);
        let closed = h.fake.record().closed.clone();
        assert!(closed.contains(&local_link()) && closed.contains(&devbox.id));

        // What would have written there before goes to memory or nowhere.
        h.devices.registry().add("dev@later").unwrap();
        h.devices.core.write_ssh();
        h.devices.app_key().unwrap();
        assert!(!data.exists());
        assert!(!cache.exists());

        // A second go finds nothing left to delete.
        block_on(h.devices.erase_app_data(vec![data.clone(), cache])).unwrap();
        assert!(!data.exists());
    }

    #[test]
    fn deleting_goes_as_far_as_it_can_and_says_what_failed() {
        let scratch = Scratch::new();
        let (data, _cache, registry) = app_folders(&scratch);
        let h = harness(registry, None);
        let keys = AppKeys::open(Box::new(Locked), None, "MaxHQ".into()).unwrap();
        h.devices.keep_app_key(keys);
        assert_eq!(
            block_on(h.devices.erase_app_data(vec![data.clone()])),
            Err(failed(
                "Couldn't forget this Mac's app key: the keychain is locked".to_owned()
            ))
        );
        assert!(!data.exists(), "the folder goes all the same");
    }

    #[test]
    fn uninstalling_with_the_data_deletes_the_apps_data_and_quits() {
        let scratch = Scratch::new();
        let (data, cache, registry) = app_folders(&scratch);
        let h = harness(registry, Some(scratch.maxd("")));
        let shell = FakeShell::default();
        shell.2.lock().unwrap().data = vec![data.clone(), cache.clone()];
        let app = App::new(h.view.clone(), h.devices, shell.clone());
        let uninstall = |delete_data| {
            block_on(app.request(&caller(), Request::UninstallLocalMaxd { delete_data }))
        };

        // Without the data, the app and its data stay.
        assert_eq!(uninstall(false), Ok(Response::UninstallLocalMaxd));
        assert_eq!(scratch.args("").as_deref(), Some("service uninstall"));
        assert!(data.join("maxhq.db").exists());
        assert_eq!(shell.2.lock().unwrap().quits, 0);

        // A maxd that fails leaves everything as it was.
        scratch.script("", "echo 'launchctl refused' >&2\nexit 1");
        assert_eq!(uninstall(true), Err(failed("launchctl refused".to_owned())));
        assert!(data.join("maxhq.db").exists());
        assert_eq!(shell.2.lock().unwrap().quits, 0);

        scratch.script("", r#"printf '%s ' "$@" > "$0.args""#);
        assert_eq!(uninstall(true), Ok(Response::UninstallLocalMaxd));
        assert_eq!(
            scratch.args("").as_deref(),
            Some("service uninstall --delete-data")
        );
        assert!(!data.exists());
        assert!(!cache.exists());
        assert_eq!(shell.2.lock().unwrap().quits, 1);
    }
}
