// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Agents that run agents, outside the session manager: the line to other
//! devices a call goes over, the access keys tools outside MaxHQ sign in
//! with, and `maxd mcp`, which serves the agent tools to such a tool.

pub mod mcp;
mod store;

use std::fs;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use maxhq_protocol::agents::{AgentMoveStep, McpCommand, PeerCall, PeerWake, ToolAnswer};
use maxhq_protocol::ids::{DeviceId, KeyId, PermissionId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::Secret;
use ring::digest::{digest, SHA256};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

use crate::secrets::files::Folder;

/// What every access key starts with.
const KEY_PREFIX: &str = "mhq";

/// How many random bytes an access key's secret has.
const SECRET_BYTES: usize = 32;

/// The folder under `maxd`'s data folder that holds the key files it mints.
pub const KEYS_DIR: &str = "keys";

/// The folder under [`KEYS_DIR`] that holds agent session keys, apart
/// from the access keys you mint.
pub const AGENT_KEYS_DIR: &str = "agents";

/// Why a call to another device didn't get there.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Unreachable(pub String);

/// A call's answer from another device, once it comes.
pub type Answer<'a> = Pin<Box<dyn Future<Output = Result<ToolAnswer, Unreachable>> + Send + 'a>>;

/// Whether a wake reached the device its caller is on.
pub type Woken<'a> = Pin<Box<dyn Future<Output = Result<(), Unreachable>> + Send + 'a>>;

/// What a step of moving an agent came to on another device.
pub type Stepped<'a> =
    Pin<Box<dyn Future<Output = Result<ActionOutcome, Unreachable>> + Send + 'a>>;

/// The line to the `maxd` on other devices. The device that gets a call
/// checks it against its own copy of the permission and runs it.
pub trait PeerCalls: Send + Sync {
    /// Sends `call` to `device` and waits for its answer.
    fn call(&self, device: DeviceId, call: PeerCall) -> Answer<'_>;

    /// Tells a caller on `device` that an agent it started here finished a
    /// turn.
    fn wake(&self, _device: DeviceId, _wake: PeerWake) -> Woken<'_> {
        Box::pin(async {
            Err(Unreachable(
                "MaxHQ can't reach other devices from here yet.".into(),
            ))
        })
    }

    /// Whether the link to `device` is up, opening it if it has to.
    fn reach(&self, _device: DeviceId) -> Woken<'_> {
        Box::pin(async {
            Err(Unreachable(
                "MaxHQ can't reach other devices from here yet.".into(),
            ))
        })
    }

    /// Asks `device` for a step of moving an agent.
    fn move_step(&self, _device: DeviceId, _step: AgentMoveStep) -> Stepped<'_> {
        Box::pin(async {
            Err(Unreachable(
                "MaxHQ can't reach other devices from here yet.".into(),
            ))
        })
    }

    /// The device that runs the MaxHQ app, when it's another one.
    fn home(&self) -> Option<DeviceId> {
        None
    }

    /// What the owner named `device` in the link book, if it's in there.
    fn name_of(&self, _device: DeviceId) -> Option<String> {
        None
    }
}

/// No line to other devices yet. Device links bring the real one.
pub struct NoPeers;

impl PeerCalls for NoPeers {
    fn call(&self, _device: DeviceId, _call: PeerCall) -> Answer<'_> {
        Box::pin(async {
            Err(Unreachable(
                "MaxHQ can't reach other devices from here yet.".into(),
            ))
        })
    }
}

/// An access key as `maxd` keeps it: never the secret, only its hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyRecord {
    /// The key.
    pub id: KeyId,
    /// The name you gave it, which names its file too.
    pub name: String,
    /// The permission it calls under.
    pub permission: PermissionId,
    /// The SHA-256 of its secret.
    #[serde(skip)]
    pub hash: Vec<u8>,
}

/// A new access key: the text a tool signs in with, and its record.
pub struct Minted {
    /// `mhq_<key id>_<secret>`.
    pub text: Secret,
    /// What `maxd` keeps.
    pub record: KeyRecord,
}

/// Makes a new access key for `permission`.
pub fn mint(name: &str, permission: PermissionId) -> io::Result<Minted> {
    let (text, key) = new_key()?;
    Ok(Minted {
        text,
        record: KeyRecord {
            id: key.id,
            name: name.to_owned(),
            permission,
            hash: key.hash,
        },
    })
}

/// A session key `maxd` gives one agent it runs, so the agent's own
/// `maxd mcp` signs in as that agent. It's an access key in all but its
/// holder, and `maxd` keeps it with the agent: never the secret, only its
/// hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKey {
    /// The key.
    pub id: KeyId,
    /// The SHA-256 of its secret.
    pub hash: Vec<u8>,
}

/// Makes a new key: `mhq_<key id>_<secret>`, and its id and hash.
pub fn new_key() -> io::Result<(Secret, AgentKey)> {
    let mut bytes = [0u8; SECRET_BYTES];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| io::Error::other("couldn't make random bytes"))?;
    let secret = URL_SAFE_NO_PAD.encode(bytes);
    let id = KeyId::generate();
    let text = Secret::new(format!("{KEY_PREFIX}_{}_{secret}", id.0.simple()));
    Ok((
        text,
        AgentKey {
            id,
            hash: hash(&secret),
        },
    ))
}

/// The key an access key's text names, and the hash of its secret, or
/// none for text that isn't an access key.
pub fn parse(text: &Secret) -> Option<(KeyId, Vec<u8>)> {
    let mut parts = text.expose().trim().splitn(3, '_');
    if parts.next()? != KEY_PREFIX {
        return None;
    }
    let id = parts.next()?.parse().ok()?;
    let secret = parts.next().filter(|secret| !secret.is_empty())?;
    Some((id, hash(secret)))
}

/// Whether two hashes match, in time that doesn't depend on where they
/// differ.
pub fn same_hash(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

/// The SHA-256 of `secret`, as keys and webhook tokens are kept.
pub(crate) fn hash(secret: &str) -> Vec<u8> {
    digest(&SHA256, secret.as_bytes()).as_ref().to_vec()
}

/// The file a key named `name` goes in: its name with anything but
/// letters, digits, dashes and underscores made a dash.
pub fn key_file(name: &str) -> String {
    let stem: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let stem = if stem.is_empty() { "key".into() } else { stem };
    format!("{stem}.key")
}

/// Writes a new key's text into `dir`, 0600 in a 0700 folder, and gives
/// its path.
pub fn write_key(dir: &Path, name: &str, text: &Secret) -> io::Result<PathBuf> {
    let file = key_file(name);
    Folder::open(dir.to_path_buf())?.write(&file, text.expose().as_bytes())?;
    Ok(dir.join(file))
}

/// Deletes a key file. One that's already gone counts as deleted.
pub fn remove_key(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// The `maxd` an MCP client runs: the path updates keep pointing at the
/// newest build, or this program when it isn't there. That's the link
/// `~/.local/bin/maxd` on Unix, and `maxd.exe` through the junction
/// `%LOCALAPPDATA%\MaxHQ\bin\current` on Windows. It's absolute, since no
/// shell reads an MCP client's config.
pub fn maxd_program() -> PathBuf {
    let var = |name| std::env::var_os(name).filter(|value| !value.is_empty());
    let stable = if cfg!(windows) {
        var("LOCALAPPDATA")
            .map(|local| PathBuf::from(local).join(maxhq_core::authorized_keys::WINDOWS_MAXD))
    } else {
        var("HOME").map(|home| PathBuf::from(home).join(".local/bin/maxd"))
    };
    stable
        .filter(|path| fs::symlink_metadata(path).is_ok())
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_else(|| PathBuf::from("maxd"))
}

/// The command that runs `maxd mcp` with the key file at `path`.
pub fn mcp_command(program: &Path, path: &Path) -> McpCommand {
    McpCommand {
        command: program.display().to_string(),
        args: vec![
            "mcp".into(),
            "--key-file".into(),
            path.display().to_string(),
        ],
    }
}

/// Reads a key file. It must be a plain file only you can read, not a
/// link.
pub fn read_key(path: &Path) -> io::Result<Secret> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() {
        return Err(io::Error::other(format!(
            "{} isn't a plain file",
            path.display()
        )));
    }
    if maxhq_os::fsmode::mode_of(&meta) & 0o077 != 0 {
        return Err(io::Error::other(format!(
            "others can read {}. Run chmod 600 on it",
            path.display()
        )));
    }
    Ok(Secret::new(fs::read_to_string(path)?.trim().to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_key_parses_back_to_its_id_and_hash() {
        let minted = mint("ci bot", PermissionId::derive("p")).expect("it mints");
        let (id, hash) = parse(&minted.text).expect("it parses");
        assert_eq!(id, minted.record.id);
        assert!(same_hash(&hash, &minted.record.hash));
        let other = mint("ci bot", PermissionId::derive("p")).expect("it mints");
        let (_, wrong) = parse(&other.text).expect("it parses");
        assert!(!same_hash(&wrong, &minted.record.hash));
        assert!(parse(&Secret::new("nope".into())).is_none());
        assert!(parse(&Secret::new(format!(
            "mhq_{}_",
            minted.record.id.0.simple()
        )))
        .is_none());
    }

    #[test]
    #[cfg(unix)]
    fn a_key_file_is_private_and_a_wide_one_is_refused() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("maxd-keys-{}", uuid::Uuid::now_v7()));
        let text = Secret::new("mhq_x_y".into());
        let path = write_key(&dir, "ci bot/1", &text).expect("it writes");
        assert_eq!(path.file_name().unwrap(), "ci-bot-1.key");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(read_key(&path).expect("it reads"), text);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_key(&path).is_err());
        let link = dir.join("link.key");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(read_key(&link).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
