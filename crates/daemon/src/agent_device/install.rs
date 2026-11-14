// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: `npm install
// --prefix` into a staging folder beside `tools/<name>/<version>`, the 10
// minute limit, the `.install-complete` sentinel that holds the version,
// the entry point checked before the rename into place, an install another
// process finished counting as done, and pruning that keeps the pin, the
// last other complete install and any a running process uses.

//! Installing the pinned agent-device into `maxd`'s own folder, and
//! removing the old ones.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::process;

/// The npm package.
pub const PACKAGE: &str = "agent-device";

/// How long `npm install` may take.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How much of npm's output a failure keeps.
const INSTALL_OUTPUT: usize = 16 * 1024;

/// The file an install writes last, holding its version.
const SENTINEL: &str = ".install-complete";

/// Where one version goes, under the tools folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// `<tools>/agent-device/<version>`.
    pub dir: PathBuf,
    /// The CLI's entry point, which `node` runs.
    pub entry: PathBuf,
}

impl Paths {
    /// Where `version` goes under `tools`.
    pub fn of(tools: &Path, version: &str) -> Self {
        let dir = tools.join(PACKAGE).join(version);
        let entry = entry_under(&dir);
        Self { dir, entry }
    }

    /// Whether `version` is installed here in full.
    fn complete(&self, version: &str) -> bool {
        self.entry.is_file() && sentinel(&self.dir).as_deref() == Some(version)
    }
}

/// The entry point under an install folder.
fn entry_under(dir: &Path) -> PathBuf {
    dir.join("node_modules")
        .join(PACKAGE)
        .join("bin")
        .join("agent-device.mjs")
}

/// The version an install folder's sentinel holds.
fn sentinel(dir: &Path) -> Option<String> {
    fs::read_to_string(dir.join(SENTINEL))
        .ok()
        .map(|text| text.trim().to_owned())
}

/// Installs `version` under `tools` with `npm`, unless it's there already.
/// The answer is why it failed, for a person to read.
pub async fn ensure(tools: &Path, npm: &Path, version: &str) -> Result<Paths, String> {
    let paths = Paths::of(tools, version);
    if paths.complete(version) {
        return Ok(paths);
    }
    let parent = paths.dir.parent().expect("a version has a parent folder");
    match fs::remove_dir_all(&paths.dir) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Err(failed("removing an incomplete install")),
    }
    fs::create_dir_all(parent).map_err(|_| failed("preparing the install folder"))?;
    let staging = parent.join(format!(".staging-{}", uuid::Uuid::now_v7().simple()));
    fs::create_dir(&staging).map_err(|_| failed("preparing the install folder"))?;
    let installed = stage(&staging, npm, version, &paths).await;
    let _ = fs::remove_dir_all(&staging);
    installed.map(|()| paths)
}

/// Why an install failed at `step`, for a person to read.
fn failed(step: &str) -> String {
    format!("Installing agent-device failed while {step}.")
}

/// Installs into `staging`, then moves it into place.
async fn stage(staging: &Path, npm: &Path, version: &str, paths: &Paths) -> Result<(), String> {
    let mut command = std::process::Command::new(npm);
    command.args(["install", "--prefix"]).arg(staging).args([
        "--no-fund".to_owned(),
        "--no-audit".to_owned(),
        format!("{PACKAGE}@{version}"),
    ]);
    let ran = process::run(command, INSTALL_TIMEOUT, INSTALL_OUTPUT)
        .await
        .map_err(|_| failed("running npm install"))?;
    if ran.exit_code != Some(0) {
        let code = ran
            .exit_code
            .map_or(String::new(), |code| format!(" (exit code {code})"));
        let line = process::last_line(&ran).map_or(String::new(), |line| format!(": {line}"));
        return Err(format!(
            "Installing agent-device failed while running npm install{code}{line}"
        ));
    }
    if !entry_under(staging).is_file() {
        return Err(failed("checking the installed entry point"));
    }
    fs::write(staging.join(SENTINEL), format!("{version}\n"))
        .map_err(|_| failed("recording the finished install"))?;
    match fs::rename(staging, &paths.dir) {
        Ok(()) => Ok(()),
        // Another process put the same version in place first.
        Err(_) if paths.complete(version) => Ok(()),
        Err(_) => Err(failed("moving the install into place")),
    }
}

/// Whether `name` reads as a version, such as `0.21.31` or
/// `0.19.0-canary-20261023`.
fn is_version(name: &str) -> bool {
    let (core, pre) = match name.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (name, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        && pre.is_none_or(|pre| {
            !pre.is_empty()
                && pre
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        })
}

/// Removes the versions under `tools` that aren't `required`, the last
/// other complete install, or one a command line in `running` names, as
/// `ps -ax -o command=` lists them. Nothing goes until `required` is
/// complete. Answers the versions it removed.
pub fn prune(tools: &Path, required: &str, running: &str) -> Vec<String> {
    let Ok(names) = fs::read_dir(tools.join(PACKAGE)) else {
        return Vec::new();
    };
    let mut complete: Vec<(String, PathBuf, SystemTime)> = names
        .filter_map(Result::ok)
        .filter_map(|item| {
            let version = item.file_name().to_str()?.to_owned();
            let dir = item.path();
            let meta = fs::symlink_metadata(&dir).ok()?;
            let done = fs::metadata(dir.join(SENTINEL)).ok()?;
            (is_version(&version)
                && meta.is_dir()
                && sentinel(&dir).as_deref() == Some(version.as_str()))
            .then(|| {
                (
                    version,
                    dir,
                    done.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                )
            })
        })
        .collect();
    if !complete.iter().any(|(version, ..)| version == required) {
        return Vec::new();
    }
    complete.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| b.0.cmp(&a.0)));
    let previous = complete
        .iter()
        .find(|(version, ..)| version != required)
        .map(|(version, ..)| version.clone());
    let mut removed = Vec::new();
    for (version, dir, _) in complete {
        let in_use = running.contains(&format!("{}/", dir.display()));
        if version == required || Some(&version) == previous.as_ref() || in_use {
            continue;
        }
        if fs::remove_dir_all(&dir).is_ok() {
            removed.push(version);
        }
    }
    removed
}

/// The command lines of the processes running now, or none when they
/// can't be listed, in which case nothing should be pruned.
pub async fn running() -> Option<String> {
    let output = tokio::process::Command::new("ps")
        .args(["-ax", "-o", "command="])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(Duration::from_secs(10), output)
        .await
        .ok()?
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    (output.status.success() && !text.is_empty()).then_some(text)
}
