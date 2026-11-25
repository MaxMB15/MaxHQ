// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The MCP servers you added yourself, for a Claude agent kept from its
//! folder's files. `--strict-mcp-config` keeps out every server the CLI
//! would load, so the ones in your account's own config come back through
//! `--mcp-config`: those at user scope, and those at local scope for this
//! project. Servers a folder names in `.mcp.json` stay out.
//!
//! The CLI keeps both in its global config, `.claude.json` in
//! `CLAUDE_CONFIG_DIR` or the home folder. User servers are under the top
//! `mcpServers`, and local ones under `projects.<root>.mcpServers`, where
//! the root is the folder's checkout, the main checkout for a linked
//! worktree, or the folder itself outside one. Only those keys are read.
//!
//! A server's settings can hold keys, so nothing here logs them, and the
//! file the CLI reads is 0600 in the runtime folder. It goes when its
//! session is forgotten, or when a later start finds the session gone.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use maxhq_core::fs_ext;
use maxhq_core::paths;
use maxhq_core::tools::SERVER;
use maxhq_hold::{Framing, Launch};
use maxhq_os::fsmode;
use maxhq_protocol::ids::SessionId;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tracing::warn;

use crate::folder_trust;

/// The flag that names the servers to load anyway.
const MCP_CONFIG: &str = "--mcp-config";

/// The folder under the runtime folder the files go in.
const FOLDER: &str = "mcp";

/// The most of a global config read. It holds every project's settings,
/// so it can grow, but not this far.
const MOST: u64 = 32 << 20;

/// How old a file whose session has no folder must be before a start
/// removes it. A start writes its file before its holder makes the folder.
const STALE: Duration = Duration::from_secs(10 * 60);

/// Where the files go, and the holders' session folders that say which
/// files are still in use.
struct Folders {
    /// The files the CLIs read.
    files: PathBuf,
    /// One folder per held session.
    sessions: PathBuf,
}

impl Folders {
    /// The folders this process uses: the device's for `maxd`, and the
    /// system ones in a guest.
    fn here() -> Option<Self> {
        Some(Self {
            files: paths::runtime_dir()?.join(FOLDER),
            sessions: paths::hold_dir()?,
        })
    }
}

/// The part of the CLI's global config read here.
#[derive(Deserialize)]
struct Config {
    #[serde(default, rename = "mcpServers")]
    servers: Value,
    #[serde(default)]
    projects: Map<String, Value>,
}

/// Gives a Claude launch kept from its folder's files the MCP servers you
/// added yourself, as the place it starts in has them. A launch whose
/// servers can't be read starts without them.
pub async fn add(launch: &mut Launch) {
    let Some(folders) = Folders::here() else {
        return;
    };
    let home = match &launch.run_as {
        Some(user) => Some(user.home.clone()),
        None => maxhq_os::home::home_dir(),
    };
    if let Some(home) = home {
        fill(launch, folders, &home).await;
    }
}

/// [`add`], with the files in `folders` and the config under `home`.
async fn fill(launch: &mut Launch, folders: Folders, home: &Path) {
    if !wants(launch) {
        return;
    }
    let config = config_file(config_dir(launch).as_deref(), home);
    let session = launch.session;
    let cwd = launch.cwd.clone();
    let owner = launch.run_as.as_ref().map(|user| (user.uid, user.gid));
    let written = tokio::task::spawn_blocking(move || {
        let servers = servers(&config, &cwd, owner.map(|(uid, _)| uid))?;
        if servers.is_empty() {
            return Ok(None);
        }
        write(&folders, session, servers, owner).map(Some)
    })
    .await
    .unwrap_or_else(|err| Err(io::Error::other(err.to_string())));
    match written {
        // Last, since the flag takes every argument after it that isn't
        // a flag.
        Ok(Some(path)) => {
            launch.args.push(MCP_CONFIG.into());
            launch.args.push(path.into_os_string());
        }
        Ok(None) => {}
        Err(err) => warn!(%session, "starting without the MCP servers you added yourself: {err}"),
    }
}

/// Removes a forgotten session's file, if it had one.
pub fn forget(session: SessionId) {
    if let Some(folders) = Folders::here() {
        remove(&folders, session);
    }
}

/// Whether `launch` is a Claude start that folder trust keeps from its
/// folder's files, and that names no servers yet.
fn wants(launch: &Launch) -> bool {
    launch.framing == Framing::ClaudeStreamJson
        && launch
            .args
            .windows(folder_trust::RESTRICTED_ARGS.len())
            .any(|args| args == folder_trust::RESTRICTED_ARGS)
        && !launch.args.iter().any(|arg| arg == MCP_CONFIG)
}

/// The config folder a launch sets, if it sets one.
fn config_dir(launch: &Launch) -> Option<PathBuf> {
    let name = OsString::from("CLAUDE_CONFIG_DIR");
    launch
        .env
        .iter()
        .rev()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| PathBuf::from(value))
}

/// The CLI's global config file for a config folder, or for the default
/// one under `home`. One that older versions left in `.config.json` wins
/// while it has anything in it, as it does for the CLI.
fn config_file(dir: Option<&Path>, home: &Path) -> PathBuf {
    let legacy = dir
        .map_or_else(|| home.join(".claude"), Path::to_path_buf)
        .join(".config.json");
    if fs::metadata(&legacy).is_ok_and(|meta| meta.is_file() && meta.len() > 0) {
        return legacy;
    }
    dir.unwrap_or(home).join(".claude.json")
}

/// Your servers for a folder: the user ones, then the local ones for its
/// project over those with the same name, less the ones you turned off
/// there and MaxHQ's own name. `owner` is the user the file must belong
/// to, when it's read for someone else.
fn servers(config: &Path, cwd: &Path, owner: Option<u32>) -> io::Result<Map<String, Value>> {
    let Some(config) = read(config, owner)? else {
        return Ok(Map::new());
    };
    let mut servers = objects(config.servers);
    let root = project_root(cwd);
    let project = config.projects.get(&root.display().to_string());
    if let Some(project) = project {
        servers.extend(objects(
            project.get("mcpServers").cloned().unwrap_or_default(),
        ));
        let off: HashSet<&str> = project
            .get("disabledMcpServers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        servers.retain(|name, _| !off.contains(name.as_str()));
    }
    servers.remove(SERVER);
    Ok(servers)
}

/// A map of servers, or none for anything else.
fn objects(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// Reads the global config, or nothing when there's none.
fn read(path: &Path, owner: Option<u32>) -> io::Result<Option<Config>> {
    let file = match fsmode::open_no_wait(OpenOptions::new().read(true)).open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::other("the config isn't a regular file"));
    }
    // Read as root for the agent's user, a link to a file of root's would
    // hand that file to the agent.
    if owner.is_some_and(|uid| fsmode::owner_of(&meta) != Some(uid)) {
        return Err(io::Error::other("the config belongs to another user"));
    }
    let mut bytes = Vec::new();
    file.take(MOST + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MOST {
        return Err(io::Error::other("the config is too big to read"));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|err| io::Error::other(format!("the config doesn't parse: {err}")))
}

/// The folder the CLI keys local servers by: the checkout's top folder,
/// the main checkout's for a linked worktree, or the folder itself
/// outside a checkout. The CLI runs in the real path, so links resolve.
fn project_root(cwd: &Path) -> PathBuf {
    let cwd = fs_ext::canonicalize(cwd).unwrap_or_else(|_| cwd.to_owned());
    let Some((level, key)) = folder_trust::checkout_of(&cwd) else {
        return cwd;
    };
    let common = PathBuf::from(key);
    match (common.file_name(), common.parent()) {
        (Some(name), Some(parent)) if name == ".git" => parent.to_owned(),
        _ => level,
    }
}

/// Writes a session's servers where its CLI can read them and no one else
/// can, owned by `owner` when that's someone else.
fn write(
    folders: &Folders,
    session: SessionId,
    servers: Map<String, Value>,
    owner: Option<(u32, u32)>,
) -> io::Result<PathBuf> {
    // The CLI may run as another user, who has to reach its own file.
    fsmode::dir_all_with_mode(&folders.files, 0o711)?;
    sweep(folders, session);
    let path = file_of(folders, session);
    match fs::remove_file(&path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
        _ => {}
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    fsmode::set_open_mode(&mut options, 0o600);
    let mut file = fs_ext::no_follow(&mut options).open(&path)?;
    fsmode::private_file(&path)?;
    if let Some((uid, gid)) = owner {
        fs_ext::chown_file(&file, uid, gid)?;
    }
    let body = serde_json::to_vec(&json!({ "mcpServers": servers }))?;
    file.write_all(&body)?;
    Ok(path)
}

/// Removes the old files of sessions whose folders are gone, which a
/// `maxd` that stopped before forgetting them leaves behind.
fn sweep(folders: &Folders, keep: SessionId) {
    let Ok(entries) = fs::read_dir(&folders.files) else {
        return;
    };
    let keep = keep.to_string();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(session) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
            continue;
        };
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .is_ok_and(|at| at.elapsed().is_ok_and(|age| age > STALE));
        if old && session != keep && !folders.sessions.join(session).exists() {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn file_of(folders: &Folders, session: SessionId) -> PathBuf {
    folders.files.join(format!("{session}.json"))
}

fn remove(folders: &Folders, session: SessionId) {
    match fs::remove_file(file_of(folders, session)) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => {
            warn!(%err, %session, "couldn't remove the agent's MCP servers file");
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
