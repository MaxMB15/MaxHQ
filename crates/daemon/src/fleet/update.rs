// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Installing a new maxd or guest binary that the app uploaded.
//!
//! maxd versions live in `~/.local/lib/maxhq/versions/<build>/maxd`, and
//! `~/.local/bin/maxd` is a symlink to the one that runs. An update checks
//! the upload, moves it into its own folder and flips the link with a
//! rename, which is atomic, so the link always names a whole binary. The
//! version before it stays, so a watcher can put it back. On Windows the
//! versions live under `%LOCALAPPDATA%\MaxHQ`, and the link is the
//! junction `bin\current` to a version's folder, as
//! [`maxhq_core::paths::MaxdLayout`] says.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use maxhq_core::fs::{write_atomic, Atomic};
use maxhq_os::link::point;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::identity::{self, BUILD_LEN};
use crate::service::{self, Manager, Runner};

/// How long `<new> --build` may take. A binary's first run on macOS
/// waits for the system's security check, which takes seconds on a busy
/// Mac.
pub const BUILD_CHECK_WAIT: Duration = Duration::from_secs(30);

/// How long the watcher waits for the new maxd to say it's healthy.
pub const HEALTHY_WAIT: Duration = Duration::from_secs(120);

/// How often the watcher looks.
pub const WATCH_POLL: Duration = Duration::from_secs(1);

/// How long a new maxd listens before it counts itself healthy without a
/// hello.
pub const LISTENING_HEALTHY: Duration = Duration::from_secs(30);

/// Where maxd's versions and update record live: under a home folder on
/// Unix, and in MaxHQ's folder, beside maxd's data, on Windows.
pub use maxhq_core::paths::MaxdLayout as Layout;

/// `update.json`: what the last update installed and whether it came up.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateState {
    /// The build that ran before, which the watcher puts back.
    pub previous: Option<String>,
    /// The build the update installed.
    pub build: String,
    /// True once the new build has served a hello or listened for a while.
    pub healthy: bool,
    /// When the update ran, in seconds since 1970.
    pub at: u64,
}

impl UpdateState {
    /// Reads `update.json`, or `None` when it's missing or unreadable.
    pub fn read(layout: &Layout) -> Option<Self> {
        let bytes = fs::read(layout.state()).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Writes `update.json` whole, through a rename.
    pub fn write(&self, layout: &Layout) -> io::Result<()> {
        let path = layout.state();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        write_atomic(&path, &bytes, Atomic::PLAIN)
    }
}

/// Why an update was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Refusal {
    /// The upload isn't there.
    #[error("the upload isn't here any more. Send it again")]
    Missing,
    /// The upload's sha256 isn't the one the app sent.
    #[error("the upload doesn't match its checksum, so it wasn't installed")]
    Checksum,
    /// No release manifest came with the upload, and it isn't marked as
    /// one you chose to send unsigned.
    #[error(
        "no release signed this upload, so it wasn't installed. Update the app, or send it again \
         from Update everything, which asks before it sends a build from source"
    )]
    Unsigned,
    /// The release manifest's signature isn't the release key's.
    #[error(
        "the release manifest's signature doesn't match the release key, so the upload wasn't \
         installed"
    )]
    Signature,
    /// The release manifest, or this maxd's own key or version, doesn't
    /// read.
    #[error("the release manifest can't be read ({0}), so the upload wasn't installed")]
    Manifest(String),
    /// The release is older than the maxd that runs.
    #[error("the release is version {release}, older than this maxd's {running}, so it wasn't installed")]
    Older {
        /// The manifest's version.
        release: String,
        /// This maxd's version.
        running: String,
    },
    /// The release lists the upload, but for another target.
    #[error("the release lists this upload as {listed}, not {wanted}, so it wasn't installed")]
    OtherTarget {
        /// The entry that lists it.
        listed: String,
        /// The entry this device needs.
        wanted: String,
    },
    /// The release doesn't list the upload at all.
    #[error("the release doesn't list this upload as {entry}, so it wasn't installed")]
    Unlisted {
        /// The entry this device needs.
        entry: String,
    },
    /// The new binary didn't run, or printed a build that isn't its own.
    #[error("the new binary didn't report its own build ({0}), so it wasn't installed")]
    Build(String),
    /// The running maxd isn't the one `~/.local/bin/maxd` names, so
    /// flipping that link wouldn't change what runs.
    #[error(
        "this maxd doesn't run from ~/.local/bin/maxd, so it can't update itself. Set it up again"
    )]
    NotLinked,
    /// This device runs no container workspaces.
    #[error("this device has no guest binary to replace")]
    NoGuests,
    /// Something on disk failed.
    #[error("couldn't install the update: {0}")]
    Io(String),
}

impl From<io::Error> for Refusal {
    fn from(err: io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

/// What an install did.
#[derive(Debug, PartialEq, Eq)]
pub enum Installed {
    /// This build already runs, so nothing changed.
    Same,
    /// The link now names `build`, and `previous` ran before.
    Flipped {
        /// The new build.
        build: String,
        /// The build before it.
        previous: String,
    },
}

/// Checks that `file` has the sha256 `wanted`, in hex.
pub fn verify(file: &Path, wanted: &str) -> Result<(), Refusal> {
    let sha = identity::sha256_of(file).map_err(|_| Refusal::Missing)?;
    if sha.eq_ignore_ascii_case(wanted.trim()) {
        Ok(())
    } else {
        Err(Refusal::Checksum)
    }
}

/// Runs `<binary> --build` and checks that it prints its own build.
/// Returns that build.
pub async fn check_build(binary: &Path, wait: Duration) -> Result<String, Refusal> {
    let expected = identity::build_of(binary)?;
    let output = match tokio::time::timeout(wait, run_build(binary)).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => return Err(Refusal::Build(format!("it didn't run: {err}"))),
        Err(_) => return Err(Refusal::Build("it took too long".to_owned())),
    };
    let printed = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if output.status.success() && printed == expected {
        Ok(expected)
    } else if printed.is_empty() {
        Err(Refusal::Build("it printed nothing".to_owned()))
    } else {
        Err(Refusal::Build(format!("it printed {printed}")))
    }
}

/// Runs `<binary> --build`, trying again while the fresh binary is busy.
async fn run_build(binary: &Path) -> io::Result<std::process::Output> {
    crate::tools::retry_busy(|| async {
        tokio::process::Command::new(binary)
            .arg("--build")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
    })
    .await
}

/// Copies `staged` to a fresh file in `dir` that only its owner may
/// change, and makes it executable.
fn copy_executable(staged: &Path, dir: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Windows runs only what it can tell is a program.
    let temp = dir.join(format!(
        ".incoming-{}-{nonce}{}",
        std::process::id(),
        std::env::consts::EXE_SUFFIX
    ));
    fs::copy(staged, &temp)?;
    maxhq_os::fsmode::set_mode(&temp, 0o755)?;
    Ok(temp)
}

/// Installs `staged` as the maxd that `layout`'s link names. `current` is
/// the running maxd, which must be the one the link names.
pub async fn install_maxd(
    layout: &Layout,
    staged: &Path,
    sha256: &str,
    current: &Path,
) -> Result<Installed, Refusal> {
    verify(staged, sha256)?;
    let link = layout.link();
    let linked = fs::canonicalize(layout.program()).map_err(|_| Refusal::NotLinked)?;
    let current = fs::canonicalize(current).map_err(|_| Refusal::NotLinked)?;
    if linked != current {
        return Err(Refusal::NotLinked);
    }
    let versions = layout.versions();
    let temp = copy_executable(staged, &versions)?;
    let build = match check_build(&temp, BUILD_CHECK_WAIT).await {
        Ok(build) => build,
        Err(refusal) => {
            let _ = fs::remove_file(&temp);
            return Err(refusal);
        }
    };
    let previous = adopt_current(layout, &link, &linked)?;
    if previous == build {
        let _ = fs::remove_file(&temp);
        return Ok(Installed::Same);
    }
    let target = layout.version(&build);
    fs::create_dir_all(target.parent().expect("a version has a folder"))?;
    fs::rename(&temp, &target)?;
    UpdateState {
        previous: Some(previous.clone()),
        build: build.clone(),
        healthy: false,
        at: now_secs(),
    }
    .write(layout)?;
    point(&link, &layout.link_target(&build))?;
    prune(layout, &[&build, &previous]);
    info!(%build, %previous, "installed a new maxd");
    Ok(Installed::Flipped { build, previous })
}

/// The build the link names now. A link that's a plain file, from before
/// versions, moves into its version folder and becomes a symlink first.
/// `linked` is the program it runs, resolved.
fn adopt_current(layout: &Layout, link: &Path, linked: &Path) -> Result<String, Refusal> {
    let build = identity::build_of(linked)?;
    let home = layout.version(&build);
    let is_symlink = fs::symlink_metadata(link)?.file_type().is_symlink();
    if is_symlink && linked == fs::canonicalize(&home).unwrap_or_default() {
        return Ok(build);
    }
    if !home.exists() {
        fs::create_dir_all(home.parent().expect("a version has a folder"))?;
        if is_symlink {
            fs::copy(linked, &home)?;
        } else {
            // The running binary keeps its open copy, so moving it is safe.
            fs::rename(linked, &home)?;
        }
    }
    point(link, &layout.link_target(&build))?;
    Ok(build)
}

/// Whether `path` is a 64-bit little-endian ELF binary with no program
/// interpreter, as the static Linux builds are. Such a maxd also runs as
/// the guest in container workspaces.
pub fn is_static_elf(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    const PT_INTERP: u32 = 3;
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut header = [0u8; 64];
    if file.read_exact(&mut header).is_err()
        || &header[..4] != b"\x7fELF"
        || header[4] != 2
        || header[5] != 1
    {
        return false;
    }
    let u16_at = |at: usize| u16::from_le_bytes([header[at], header[at + 1]]);
    let mut offset = [0u8; 8];
    offset.copy_from_slice(&header[32..40]);
    let table = u64::from_le_bytes(offset);
    let entry = u64::from(u16_at(54));
    let count = u16_at(56);
    if entry < 4 || file.seek(SeekFrom::Start(table)).is_err() {
        return false;
    }
    let mut row = vec![0u8; usize::from(u16_at(54))];
    for _ in 0..count {
        if file.read_exact(&mut row).is_err() {
            return false;
        }
        if u32::from_le_bytes([row[0], row[1], row[2], row[3]]) == PT_INTERP {
            return false;
        }
    }
    true
}

/// Removes every version folder except `keep`.
fn prune(layout: &Layout, keep: &[&str]) {
    let Ok(entries) = fs::read_dir(layout.versions()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if keep.contains(&name.as_ref()) || name.len() != BUILD_LEN {
            continue;
        }
        if let Err(err) = fs::remove_dir_all(entry.path()) {
            warn!(%err, version = %name, "couldn't remove an old maxd");
        }
    }
}

/// Installs `staged` over the guest binary at `guest`.
pub async fn install_guest(staged: &Path, sha256: &str, guest: &Path) -> Result<String, Refusal> {
    verify(staged, sha256)?;
    let dir = guest.parent().ok_or(Refusal::NoGuests)?;
    let temp = copy_executable(staged, dir)?;
    let build = match check_build(&temp, BUILD_CHECK_WAIT).await {
        Ok(build) => build,
        Err(refusal) => {
            let _ = fs::remove_file(&temp);
            return Err(refusal);
        }
    };
    fs::rename(&temp, guest)?;
    info!(%build, "installed a new guest binary");
    Ok(build)
}

/// Marks `build` healthy in `update.json` when it's the build the last
/// update installed and it isn't marked yet. Returns true when it wrote.
pub fn mark_healthy(layout: &Layout, build: &str) -> bool {
    let Some(mut state) = UpdateState::read(layout) else {
        return false;
    };
    if state.build != build || state.healthy {
        return false;
    }
    state.healthy = true;
    match state.write(layout) {
        Ok(()) => {
            info!(%build, "this update is healthy");
            true
        }
        Err(err) => {
            warn!(%err, "couldn't record that this update is healthy");
            false
        }
    }
}

/// Whether `build` was just installed and still has to show it's healthy.
#[cfg(test)]
pub fn awaiting_health(layout: &Layout, build: &str) -> bool {
    UpdateState::read(layout).is_some_and(|state| state.build == build && !state.healthy)
}

/// Restarts the maxd service. Tests use a fake.
pub trait Services {
    /// Restarts the service so it runs what the link names.
    fn restart(&mut self) -> io::Result<()>;
}

/// The service manager on this system, through `launchctl` or
/// `systemctl --user`.
pub struct SystemServices<R: Runner> {
    /// launchd or systemd.
    pub manager: Manager,
    /// Your user ID, for launchd.
    pub uid: u32,
    /// What runs the commands.
    pub runner: R,
}

impl<R: Runner> Services for SystemServices<R> {
    fn restart(&mut self) -> io::Result<()> {
        let (program, args) = maxhq_core::service::restart_command(self.manager, self.uid);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let ran = self.runner.run(program, &args)?;
        if ran.success {
            Ok(())
        } else {
            Err(io::Error::other(ran.stderr.trim().to_owned()))
        }
    }
}

/// How a watch ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Watched {
    /// The new build said it's healthy.
    Healthy,
    /// Another update, or something else, replaced the one being watched.
    Superseded,
    /// The new build never said it's healthy, so the old one is back.
    RolledBack {
        /// The build that's back.
        previous: String,
    },
    /// There was nothing to put back.
    NoPrevious,
}

/// Waits up to `wait` for `build` to be marked healthy. When it isn't,
/// points the link back at the previous build and restarts the service,
/// unless the link no longer leads to `build`.
pub fn watch(
    layout: &Layout,
    build: &str,
    wait: Duration,
    poll: Duration,
    services: &mut dyn Services,
) -> io::Result<Watched> {
    let deadline = Instant::now() + wait;
    loop {
        match UpdateState::read(layout) {
            Some(state) if state.build != build => return Ok(Watched::Superseded),
            Some(state) if state.healthy => return Ok(Watched::Healthy),
            _ => {}
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(poll.min(deadline.saturating_duration_since(Instant::now())));
    }
    let Some(previous) = UpdateState::read(layout).and_then(|state| state.previous) else {
        return Ok(Watched::NoPrevious);
    };
    let target = layout.version(&previous);
    if !target.exists() {
        return Ok(Watched::NoPrevious);
    }
    if replaced(&layout.program(), &layout.version(build)) {
        // Putting the old build back would undo whatever did that.
        return Ok(Watched::Superseded);
    }
    warn!(%build, %previous, "the new maxd didn't come up, so the old one goes back");
    point(&layout.link(), &layout.link_target(&previous))?;
    UpdateState {
        previous: None,
        build: previous.clone(),
        healthy: true,
        at: now_secs(),
    }
    .write(layout)?;
    services.restart()?;
    Ok(Watched::RolledBack { previous })
}

/// Whether `link`, the program through the link, now resolves somewhere
/// other than `ours`, so something else replaced it. A link that resolves
/// nowhere counts as not replaced.
fn replaced(link: &Path, ours: &Path) -> bool {
    fs::canonicalize(link).is_ok_and(|now| fs::canonicalize(ours).ok() != Some(now))
}

/// Starts `<previous> update watch --build <build>` so it outlives this
/// maxd. On Linux it goes in a user scope, as holders do. On macOS the
/// LaunchAgent abandons its process group, so a new group is enough.
pub fn start_watcher(previous: &Path, build: &str) -> io::Result<()> {
    let mut command = if cfg!(target_os = "linux") && service::find_program("systemd-run").is_some()
    {
        let mut command = std::process::Command::new("systemd-run");
        command
            .args(["--user", "--scope", "--collect", "--quiet"])
            .arg(format!("--unit=maxd-update-watch-{build}"))
            .arg("--")
            .arg(previous);
        command
    } else {
        std::process::Command::new(previous)
    };
    command
        .args(["update", "watch", "--build", build])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    // The child is never waited for. It lives on after this maxd exits.
    maxhq_os::proc::spawn_detached(&mut command)
}

/// Seconds since 1970.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests;
