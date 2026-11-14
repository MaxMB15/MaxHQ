// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Landing a project's devflow pack in one checkout, on this device or in
//! a container's guest: a copy of its tree in `.devflow-vendor`, built
//! where the agent can't reach it and renamed into place, then its
//! `install.sh` run once per tree from the checkout's root. maxd notes what
//! it landed in its data folder, because a repository can commit a marker, an
//! installer and a `.venv`. Only a copy that note vouches for hands its
//! `.venv` to the next one, and what the installer makes is never touched.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{lchown, DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use maxhq_core::fs::{write_atomic, Atomic};
use maxhq_core::skills::disk::DiskFs;
use maxhq_core::skills::pack::{self, Marker, Next, Ran, INSTALLER, MARKER, PACK_DIR};
use maxhq_core::skills::tree;
use maxhq_protocol::skills::{PackState, TreeHash};

use super::apply;
use maxhq_hold::RunAs;
use serde::{Deserialize, Serialize};

/// How long the installer may run, under the guest's request wait.
pub const TIMEOUT: Duration = Duration::from_secs(90);

/// The folder the installers' logs go in, from a data folder.
pub const LOGS: &str = "pack-logs";

/// How many of the log's last lines a failure keeps.
const LINES: usize = 20;

/// The most a marker holds. An older maxd kept a failure's log lines in it.
const MARKER_LIMIT: u64 = 1024 * 1024;

/// The folder the uv installer puts its tools in, from a home.
const LOCAL_BIN: &str = ".local/bin";

/// One checkout's pack, as [`land`] puts it in.
pub struct Landing<'a> {
    /// The checkout's root.
    pub root: &'a Path,
    /// The pack's tree, as a folder to copy from.
    pub from: &'a Path,
    /// The tree it should be.
    pub tree: &'a TreeHash,
    /// The folder in maxd's data that holds the installer's log and maxd's
    /// note of the pack, one each per checkout, and a container's copy while
    /// it's built.
    pub logs: &'a Path,
    /// Who the installer runs as and owns the copy, in a container.
    pub user: Option<&'a RunAs>,
    /// Whether a failed install runs again.
    pub retry: bool,
    /// How long the installer may run.
    pub timeout: Duration,
}

/// What the lines `.git/info/exclude` needs for a checkout at `root`
/// whose lock has a pack: the installer's, unless someone else's pack is
/// there.
pub fn exclude_lines(root: &Path) -> Vec<String> {
    match foreign(root) {
        true => Vec::new(),
        false => pack::exclude_lines(),
    }
}

/// Whether the checkout at `root` has a `.devflow-vendor` MaxHQ didn't
/// put there.
pub fn foreign(root: &Path) -> bool {
    let vendor = root.join(PACK_DIR);
    vendor.symlink_metadata().is_ok() && marker(&vendor).is_none()
}

/// Puts the pack in and installs it, as far as it needs. `installing`
/// runs just before the installer does.
pub fn land(landing: &Landing, installing: impl FnOnce()) -> PackState {
    let vendor = landing.root.join(PACK_DIR);
    let there = vendor.symlink_metadata().is_ok();
    let vendored = foreign(landing.root) || (there && tracked(landing.root, landing.user));
    let ours = there.then(|| noted(landing.logs, landing.root)).flatten();
    let failed = |err: String| PackState::Failed {
        tree: landing.tree.clone(),
        lines: vec![err],
    };
    match pack::next(vendored, ours.as_ref(), landing.tree, landing.retry) {
        Next::Leave(state) => return state,
        Next::Copy => {
            if let Err(err) = copy(landing, &vendor, ours.is_some()) {
                return failed(format!("couldn't copy the pack: {err}"));
            }
        }
    }
    let pending = Marker {
        tree: landing.tree.clone(),
        ran: Ran::Pending,
        lines: Vec::new(),
    };
    if let Err(err) = write_note(landing.logs, landing.root, &pending) {
        return failed(format!("couldn't note the install: {err}"));
    }
    installing();
    let ran = install(landing);
    let marker = Marker {
        tree: landing.tree.clone(),
        ran: if ran.is_ok() {
            Ran::Installed
        } else {
            Ran::Failed
        },
        lines: ran.err().unwrap_or_default(),
    };
    // Only the note hears how it went. The installer had the checkout for its
    // whole run, so `vendor` could lead anywhere by now.
    match write_note(landing.logs, landing.root, &marker) {
        Ok(()) => marker.state(),
        Err(err) => failed(format!("couldn't note the install: {err}")),
    }
}

/// Takes MaxHQ's copy out of the checkout at `root`, with the installer's
/// links into it, its log and the pack's exclude lines. The files the
/// installer wrote stay, since you may have changed them. Someone else's
/// pack stays, and reads as vendored. `guest` is the work folder and the
/// user a caller running as root does this for.
pub fn remove(
    root: &Path,
    logs: &Path,
    guest: Option<(&Path, &RunAs)>,
) -> io::Result<Option<PackState>> {
    let user = guest.map(|(_, user)| user);
    let vendor = root.join(PACK_DIR);
    if vendor.symlink_metadata().is_err() {
        return clear_note(logs, root).map(|()| None);
    }
    if marker(&vendor).is_none() || tracked(root, user) {
        return Ok(Some(PackState::Vendored));
    }
    clear_note(logs, root)?;
    let links = links_into(root, &vendor);
    apply::clear(&vendor)?;
    for link in &links {
        apply::clear(link)?;
    }
    for base in LINKED {
        let base = root.join(base);
        // A folder that's a link may be shared, so it and what's in it stay.
        if !base.symlink_metadata().is_ok_and(|meta| meta.is_dir()) {
            continue;
        }
        for sub in ["skills", "agents"] {
            let dir = base.join(sub);
            if links
                .iter()
                .any(|link| link.parent() == Some(dir.as_path()))
            {
                remove_if_empty(&dir)?;
            }
        }
        if links.iter().any(|link| link.starts_with(&base)) {
            remove_if_empty(&base)?;
        }
    }
    match guest {
        Some((work, user)) => {
            apply::unexclude_as(work, root, &pack::exclude_lines(), (user.uid, user.gid))?
        }
        None => apply::unexclude(root, &pack::exclude_lines(), None)?,
    };
    Ok(None)
}

/// The folders the installer links skills and agents into, from the
/// checkout's root, each with a `skills` and an `agents` folder.
const LINKED: [&str; 4] = [".claude", ".cursor", ".github", ".codex"];

/// The links in the installer's folders that lead into `vendor`. A real
/// file, or a link that leads anywhere else, isn't one.
fn links_into(root: &Path, vendor: &Path) -> Vec<PathBuf> {
    let Ok(vendor) = fs::canonicalize(vendor) else {
        return Vec::new();
    };
    let mut links = Vec::new();
    for base in LINKED {
        for sub in ["skills", "agents"] {
            let Ok(entries) = fs::read_dir(root.join(base).join(sub)) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                let is_link = entry.file_type().is_ok_and(|kind| kind.is_symlink());
                if is_link && fs::canonicalize(&path).is_ok_and(|to| to.starts_with(&vendor)) {
                    links.push(path);
                }
            }
        }
    }
    links
}

/// Removes the folder at `dir` when nothing is left in it.
fn remove_if_empty(dir: &Path) -> io::Result<()> {
    match fs::read_dir(dir) {
        Ok(mut entries) => match entries.next() {
            None => fs::remove_dir(dir),
            Some(_) => Ok(()),
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

/// Where the checkout at `root` stands with no pack locked: someone
/// else's pack, or none.
pub fn unlocked(root: &Path) -> Option<PackState> {
    foreign(root).then_some(PackState::Vendored)
}

/// The marker in `vendor`, which only says MaxHQ put the copy there. An agent
/// can put anything at its name, so the open doesn't follow a link or wait on
/// a FIFO, only a regular file counts, and the read stops at the limit.
fn marker(vendor: &Path) -> Option<Marker> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(vendor.join(MARKER))
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    file.take(MARKER_LIMIT + 1).read_to_string(&mut text).ok()?;
    if text.len() as u64 > MARKER_LIMIT {
        return None;
    }
    serde_json::from_str(&text).ok()
}

/// Whether git tracks the pack's place in the checkout at `root`, so that the
/// repository vendors it, marker or not. An answer git can't give is no.
fn tracked(root: &Path, user: Option<&RunAs>) -> bool {
    crate::guest::worktree::git(user)
        .arg("-C")
        .arg(root)
        .args(["-c", "core.fsmonitor=false"])
        .args(["ls-files", "--", PACK_DIR])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|output| output.status.success() && !output.stdout.is_empty())
}

/// maxd's note of the pack it last put in a checkout.
#[derive(Serialize, Deserialize)]
struct Note {
    /// The checkout, since two can share a log's name.
    root: String,
    /// The tree it copied and how its install went.
    #[serde(flatten)]
    marker: Marker,
}

/// Where the note for the checkout at `root` goes, beside its log.
fn note_path(logs: &Path, root: &Path) -> PathBuf {
    log_path(logs, root).with_extension("json")
}

/// What maxd last landed in the checkout at `root`, if it has.
fn noted(logs: &Path, root: &Path) -> Option<Marker> {
    let text = fs::read_to_string(note_path(logs, root)).ok()?;
    let note: Note = serde_json::from_str(&text).ok()?;
    (note.root == root.display().to_string()).then_some(note.marker)
}

fn write_note(logs: &Path, root: &Path, marker: &Marker) -> io::Result<()> {
    fs::create_dir_all(logs)?;
    let note = Note {
        root: root.display().to_string(),
        marker: marker.clone(),
    };
    let text = serde_json::to_vec_pretty(&note).map_err(io::Error::other)?;
    write_atomic(&note_path(logs, root), &text, Atomic::PLAIN)
}

/// Forgets the pack of the checkout at `root`: maxd's note and the
/// installer's log.
fn clear_note(logs: &Path, root: &Path) -> io::Result<()> {
    apply::clear(&note_path(logs, root))?;
    apply::clear(&log_path(logs, root))
}

/// Builds the copy in a folder only maxd can enter, checks its tree, hands it
/// to its owner there and renames it into place last, marked pending. Root
/// walks the copy by path, so the agent can't get at it until that's done.
/// `ours` carries the old copy's `.venv` over, for a real folder that maxd's
/// note says it made. Any other `.venv` stays out, since the installer runs
/// the python it finds there. Whatever else is at `vendor`, a link too, goes
/// without being followed.
fn copy(landing: &Landing, vendor: &Path, ours: bool) -> io::Result<()> {
    let hold = hold(landing, vendor)?;
    let temp = hold.join("copy");
    let landed = build(landing, &temp).and_then(|()| {
        let venv = vendor.join(".venv");
        let real = |path: &Path| path.symlink_metadata().is_ok_and(|meta| meta.is_dir());
        let carry = ours && real(vendor) && real(&venv);
        let old = apply::beside(vendor, "old");
        let there = vendor.symlink_metadata().is_ok();
        if there {
            fs::rename(vendor, &old)?;
        }
        if let Err(err) = fs::rename(&temp, vendor) {
            if there {
                let _ = fs::rename(&old, vendor);
            }
            return Err(err);
        }
        let carried = match carry {
            true => fs::rename(old.join(".venv"), &venv),
            false => Ok(()),
        };
        let cleared = apply::clear(&old);
        carried.and(cleared)
    });
    let cleared = apply::clear(&hold);
    landed.and(cleared)
}

/// Makes the folder a copy is built in, which only maxd can enter. In a
/// container it's in maxd's data folder, on the container's one disk: the agent
/// owns the checkout, so it could rename even a private folder there and
/// root would follow the name. On a host maxd is the agent's own user, and the
/// checkout may be on another disk, so it stays beside `vendor`.
fn hold(landing: &Landing, vendor: &Path) -> io::Result<PathBuf> {
    let hold = match landing.user {
        Some(_) => {
            fs::create_dir_all(landing.logs)?;
            apply::beside(&landing.logs.join("pack"), "copy")
        }
        None => apply::beside(vendor, "tmp"),
    };
    DirBuilder::new().mode(0o700).create(&hold)?;
    Ok(hold)
}

fn build(landing: &Landing, temp: &Path) -> io::Result<()> {
    apply::copy_dir(landing.from, temp)?;
    apply::set_modes(temp, false)?;
    let tree = tree::of(&DiskFs, temp).map_err(io::Error::other)?;
    if tree != *landing.tree {
        return Err(io::Error::other("the pack changed while it was copied"));
    }
    let marker = Marker {
        tree: tree.clone(),
        ran: Ran::Pending,
        lines: Vec::new(),
    };
    let text = serde_json::to_vec_pretty(&marker).map_err(io::Error::other)?;
    fs::write(temp.join(MARKER), text)?;
    if let Some(owner) = owner(landing.user) {
        own_all(temp, owner)?;
    }
    Ok(())
}

/// Gives everything under `dir` to `owner`.
fn own_all(dir: &Path, (uid, gid): (u32, u32)) -> io::Result<()> {
    lchown(dir, Some(uid), Some(gid))?;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            own_all(&entry.path(), (uid, gid))?;
        } else {
            lchown(entry.path(), Some(uid), Some(gid))?;
        }
    }
    Ok(())
}

fn owner(user: Option<&RunAs>) -> Option<(u32, u32)> {
    user.map(|user| (user.uid, user.gid))
}

/// The log of the checkout at `root`.
pub fn log_path(logs: &Path, root: &Path) -> PathBuf {
    let name = root.display().to_string();
    logs.join(format!(
        "{}.log",
        name.trim_start_matches('/').replace('/', "-")
    ))
}

/// Runs the installer from the checkout's root, with the pull off, in a
/// process group of its own that goes past the timeout. A failure gives
/// the log's last lines.
fn install(landing: &Landing) -> Result<(), Vec<String>> {
    use std::os::unix::process::CommandExt;

    let log = log_path(landing.logs, landing.root);
    let opened = fs::create_dir_all(landing.logs).and_then(|()| File::create(&log));
    let file = opened.map_err(|err| vec![format!("couldn't open the install log: {err}")])?;
    let err_file = file
        .try_clone()
        .map_err(|err| vec![format!("couldn't open the install log: {err}")])?;
    // bash reads the script rather than running the file, which Linux
    // can refuse while the copy is still open somewhere.
    let mut command = Command::new("bash");
    if let Some(user) = landing.user {
        let path = format!(
            "{}:{}",
            user.home.join(LOCAL_BIN).display(),
            crate::guest::worktree::PATH
        );
        command
            .env_clear()
            .env("PATH", path)
            .env("HOME", &user.home)
            .env("USER", &user.user)
            .env("LOGNAME", &user.user)
            .uid(user.uid)
            .gid(user.gid);
    }
    command
        .arg(format!("{PACK_DIR}/{INSTALLER}"))
        .current_dir(landing.root)
        .env("DEVFLOW_SKIP_PULL", "1")
        .env("DEVFLOW_SKIP_GRAPH", "1")
        .stdin(Stdio::null())
        .stdout(file)
        .stderr(err_file)
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|err| vec![format!("couldn't start the installer: {err}")])?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < landing.timeout => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                if let Some(pid) = rustix::process::Pid::from_raw(child.id() as i32) {
                    let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let mut lines = last_lines(&log);
    match status {
        Some(status) if status.success() => Ok(()),
        Some(status) => {
            lines.push(format!("The installer stopped with {status}."));
            Err(lines)
        }
        None => {
            let secs = landing.timeout.as_secs_f32();
            lines.push(format!("The installer ran past {secs} s and was stopped."));
            Err(lines)
        }
    }
}

/// The last lines of the log at `path`.
fn last_lines(path: &Path) -> Vec<String> {
    let bytes = fs::read(path).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let from = lines.len().saturating_sub(LINES);
    lines[from..]
        .iter()
        .map(|line| (*line).to_owned())
        .collect()
}

#[cfg(test)]
mod tests;
