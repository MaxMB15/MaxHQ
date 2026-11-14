// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the registry's
// bounds on each field, an entry that breaks them dropped on its own, an
// npx or uvx package only at an exact version, and a binary for the target
// first, then npx, then uvx. The agents MaxHQ has its own drivers for stay
// hidden.

//! The ACP registry's index: what one fetch of `registry.json` holds, the
//! entries that pass its checks, and how one installs on a target.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where the registry lives.
pub const URL: &str = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";
/// The largest index read.
pub const MAX_BYTES: usize = 1024 * 1024;
/// The most entries an index holds.
const MAX_AGENTS: usize = 512;
/// The entries MaxHQ runs as its own agents, not through the registry.
pub const NATIVE: [&str; 3] = ["claude-acp", "codex-acp", "cursor"];

/// The registry's entries, as one fetch found them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    /// The index's own version.
    pub version: String,
    /// The entries that passed, without the hidden ones.
    pub agents: Vec<Entry>,
}

/// One agent the registry lists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Its ID.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its version.
    pub version: String,
    /// What it is.
    #[serde(default)]
    pub description: String,
    /// The ways it installs.
    pub distribution: Distribution,
}

/// The ways an entry installs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Distribution {
    /// An archive for each target, such as `darwin-aarch64`.
    #[serde(default)]
    pub binary: BTreeMap<String, Binary>,
    /// An npm package.
    pub npx: Option<Package>,
    /// A PyPI package.
    pub uvx: Option<Package>,
}

/// An archive for one target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binary {
    /// Where to download it.
    pub archive: String,
    /// The command in it, as a path inside the archive.
    pub cmd: String,
    /// The archive's SHA-256, when the registry gives one.
    pub sha256: Option<String>,
    /// Arguments for the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Variables for the command.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// A package with its exact version, such as `name@1.2.3`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    /// The package and its version.
    pub package: String,
    /// Arguments for the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Variables for the command.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// How an entry installs on one target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// From where.
    pub kind: Kind,
    /// Arguments for the command.
    pub args: Vec<String>,
    /// Variables for the command.
    pub env: BTreeMap<String, String>,
}

/// Where a choice installs from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An archive.
    Binary {
        /// Where to download it.
        archive: String,
        /// The command's file name.
        command: String,
        /// The archive's SHA-256, when known.
        sha256: Option<String>,
    },
    /// An npm package.
    Npx {
        /// The package.
        name: String,
        /// Its version.
        version: String,
    },
    /// A PyPI package.
    Uvx {
        /// The package.
        name: String,
        /// Its version.
        version: String,
    },
}

/// Reads an index, dropping each entry that breaks the registry's bounds
/// and the hidden ones. The index as a whole must be the registry's shape.
pub fn parse(text: &str) -> Result<Index, String> {
    if text.len() > MAX_BYTES {
        return Err("the ACP registry is larger than 1 MiB".to_owned());
    }
    let value: Value = serde_json::from_str(text)
        .map_err(|err| format!("the ACP registry doesn't parse: {err}"))?;
    let version = value
        .get("version")
        .and_then(Value::as_str)
        .filter(|version| bounded_version(version))
        .ok_or("the ACP registry has no version")?;
    let agents = value
        .get("agents")
        .and_then(Value::as_array)
        .filter(|agents| agents.len() <= MAX_AGENTS)
        .ok_or("the ACP registry lists no agents")?;
    let agents = agents
        .iter()
        .filter_map(|agent| serde_json::from_value::<Entry>(agent.clone()).ok())
        .filter_map(checked)
        .filter(|entry| !NATIVE.contains(&entry.id.as_str()))
        .collect();
    Ok(Index {
        version: version.to_owned(),
        agents,
    })
}

/// The registry's name for a Rust target, such as `darwin-aarch64` for
/// `aarch64-apple-darwin`.
pub fn registry_target(target: &str) -> Option<String> {
    let arch = target.split('-').next()?;
    if !matches!(arch, "aarch64" | "x86_64") {
        return None;
    }
    let os = if target.contains("apple-darwin") {
        "darwin"
    } else if target.contains("linux") {
        "linux"
    } else {
        return None;
    };
    Some(format!("{os}-{arch}"))
}

/// How `entry` installs on `target`, a Rust target: its archive for the
/// target, else its npm package, else its PyPI one.
pub fn choose(entry: &Entry, target: &str) -> Option<Choice> {
    let binary = registry_target(target).and_then(|name| entry.distribution.binary.get(&name));
    if let Some(binary) = binary {
        let command = (*command_path(&binary.cmd)?.last()?).to_owned();
        return Some(Choice {
            kind: Kind::Binary {
                archive: binary.archive.clone(),
                command,
                sha256: binary.sha256.as_ref().map(|sha| sha.to_ascii_lowercase()),
            },
            args: binary.args.clone(),
            env: binary.env.clone(),
        });
    }
    if let Some(npx) = &entry.distribution.npx {
        let (name, version) = npx.package.rsplit_once('@')?;
        return Some(Choice {
            kind: Kind::Npx {
                name: name.to_owned(),
                version: unprefixed(version),
            },
            args: npx.args.clone(),
            env: npx.env.clone(),
        });
    }
    let uvx = entry.distribution.uvx.as_ref()?;
    let (name, version) = uvx
        .package
        .rsplit_once("==")
        .or_else(|| uvx.package.rsplit_once('@'))?;
    Some(Choice {
        kind: Kind::Uvx {
            name: name.to_owned(),
            version: unprefixed(version),
        },
        args: uvx.args.clone(),
        env: uvx.env.clone(),
    })
}

/// A version without a leading `v`.
fn unprefixed(version: &str) -> String {
    version
        .strip_prefix(['v', 'V'])
        .unwrap_or(version)
        .to_owned()
}

/// `entry` when each field is within the registry's bounds, with its name
/// trimmed.
fn checked(mut entry: Entry) -> Option<Entry> {
    entry.name = entry.name.trim().to_owned();
    let id_ok = entry.id.len() <= 128 && lower_name(&entry.id);
    let fields_ok = !entry.name.is_empty()
        && entry.name.chars().count() <= 160
        && bounded_version(&entry.version)
        && entry.description.chars().count() <= 1024;
    let dist = &entry.distribution;
    let binaries_ok = dist.binary.values().all(|binary| {
        https(&binary.archive)
            && binary.cmd.len() <= 1024
            && command_path(&binary.cmd).is_some()
            && binary.sha256.as_deref().is_none_or(sha256)
            && extras(&binary.args, &binary.env)
    });
    let npx_ok = dist
        .npx
        .as_ref()
        .is_none_or(|npx| exact_npx(&npx.package) && extras(&npx.args, &npx.env));
    let uvx_ok = dist
        .uvx
        .as_ref()
        .is_none_or(|uvx| exact_uvx(&uvx.package) && extras(&uvx.args, &uvx.env));
    (id_ok && fields_ok && binaries_ok && npx_ok && uvx_ok).then_some(entry)
}

/// Whether arguments and variables are within bounds.
fn extras(args: &[String], env: &BTreeMap<String, String>) -> bool {
    args.len() <= 64
        && args.iter().all(|arg| arg.len() <= 1024)
        && env
            .iter()
            .all(|(name, value)| name.len() <= 256 && value.len() <= 1024)
}

/// A name of lowercase letters, digits, `.`, `_` and `-`, starting with a
/// letter or digit.
fn lower_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

/// A version that's safe as a folder name.
pub(crate) fn bounded_version(text: &str) -> bool {
    let mut chars = text.chars();
    text.len() <= 128
        && chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

/// An absolute HTTPS URL with no user or password in it.
fn https(text: &str) -> bool {
    text.len() <= 2048
        && url::Url::parse(text).is_ok_and(|url| {
            url.scheme() == "https" && url.username().is_empty() && url.password().is_none()
        })
}

/// A SHA-256 in hex.
fn sha256(text: &str) -> bool {
    text.len() == 64 && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// The command's path inside its archive, split, when it stays inside.
fn command_path(cmd: &str) -> Option<Vec<&str>> {
    let cmd = cmd.trim();
    let cmd = cmd.strip_prefix("./").unwrap_or(cmd);
    let drive = cmd.as_bytes().get(1) == Some(&b':');
    if cmd.starts_with('/') || cmd.contains('\\') || drive {
        return None;
    }
    let segments: Vec<&str> = cmd.split('/').filter(|part| !part.is_empty()).collect();
    let safe = !segments.is_empty() && segments.iter().all(|part| !matches!(*part, "." | ".."));
    safe.then_some(segments)
}

/// A release version: `1.2.3`, with an optional `v`, prerelease and build.
fn exact_version(text: &str) -> bool {
    let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
    let (text, build) = match text.split_once('+') {
        Some((text, build)) => (text, Some(build)),
        None => (text, None),
    };
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (text, None),
    };
    let tag = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
    };
    let numbers: Vec<&str> = core.split('.').collect();
    numbers.len() == 3
        && numbers
            .iter()
            .all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        && pre.is_none_or(tag)
        && build.is_none_or(tag)
}

/// A package name as PyPI or unscoped npm writes it, in any case.
fn plain_package(name: &str) -> bool {
    lower_name(&name.to_ascii_lowercase())
}

/// An npm package at an exact version: `name@1.2.3` or `@scope/name@1.2.3`.
fn exact_npx(spec: &str) -> bool {
    let Some((name, version)) = spec.rsplit_once('@') else {
        return false;
    };
    let named = match name.strip_prefix('@') {
        Some(scoped) => scoped.split_once('/').is_some_and(|(scope, rest)| {
            !scope.is_empty()
                && !rest.is_empty()
                && !scope.contains(['@', '/'])
                && !rest.contains('@')
                && !spec.contains(char::is_whitespace)
        }),
        None => plain_package(name),
    };
    spec.len() <= 256 && named && exact_version(version)
}

/// A PyPI package at an exact version: `name==1.2.3` or `name@1.2.3`.
fn exact_uvx(spec: &str) -> bool {
    let split = spec.rsplit_once("==").or_else(|| spec.rsplit_once('@'));
    spec.len() <= 256
        && split.is_some_and(|(name, version)| plain_package(name) && exact_version(version))
}
