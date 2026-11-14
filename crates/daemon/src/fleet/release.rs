// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Checking that an uploaded maxd or guest binary is a release's before
//! anything runs or installs it.
//!
//! A release carries `maxd-release.json`, which lists its version and the
//! sha256 of each `maxd-<target>`, and `maxd-release.json.sig`, a minisign
//! signature over it made with the release key. The app sends both with
//! the upload. [`vouch`] checks, in order: the upload's checksum against
//! the one the app sent, the manifest's signature against
//! [`RELEASE_PUBLIC_KEY`], that the manifest's version isn't older than
//! this maxd's, and that the manifest lists the upload's sha256 under the
//! entry for this device's target. A build from source has no manifest, so
//! it goes through only when you chose to send it on the Mac, which marks
//! the upload unsigned.

use std::collections::BTreeMap;
use std::path::Path;

use base64::Engine;
use maxhq_protocol::message::{SignedRelease, UpdateKind};
use minisign_verify::{PublicKey, Signature};
use serde::Deserialize;
use tracing::warn;

use super::update::{self, Refusal};

/// The release key's public half: base64 of the minisign public key text,
/// as `tauri signer generate` writes it. It must equal
/// `plugins.updater.pubkey` in `apps/desktop/src-tauri/tauri.conf.json`,
/// since one key signs both the app's own updates and the fleet manifest.
pub const RELEASE_PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEY2NkZENzhDMUZBQzU5NUMKUldSY1dhd2ZqTmR2OWs3dStvNGt1RE05K0RtTjYvMk16MnhXUis1ZXhpTG1kMjF1YkFIQUR1bzUK";

/// What an upload is checked against: this maxd's own release key, target
/// and version.
#[derive(Clone, Copy, Debug)]
pub struct Trust<'a> {
    /// The release key, as [`RELEASE_PUBLIC_KEY`] gives it.
    pub key: &'a str,
    /// The Rust target this maxd was built for.
    pub target: &'a str,
    /// This maxd's version, which a release mustn't be older than.
    pub version: &'a str,
}

impl Trust<'static> {
    /// This maxd's own: the release key, its target and its version.
    pub fn own() -> Self {
        Self {
            key: RELEASE_PUBLIC_KEY,
            target: crate::identity::TARGET,
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

/// `maxd-release.json`.
#[derive(Debug, Deserialize)]
struct Manifest {
    /// The release's version, such as `0.2.0`.
    version: String,
    /// The sha256 in hex of each binary, by `maxd-<target>`.
    binaries: BTreeMap<String, String>,
}

/// The manifest entry an upload of `kind` must be listed under, on a
/// device whose maxd was built for `target`. The static musl maxd doubles
/// as the guest binary, so a guest checks the musl entry for this
/// device's architecture.
pub fn entry_for(kind: UpdateKind, target: &str) -> String {
    match kind {
        UpdateKind::Maxd => format!("maxd-{target}"),
        UpdateKind::Guest => {
            let arch = target.split('-').next().unwrap_or(target);
            format!("maxd-{arch}-unknown-linux-musl")
        }
    }
}

/// Checks the upload at `staged` before anything runs or installs it: its
/// checksum against `sha256`, then `release` as [`check`] does. Without a
/// release only an upload marked `unsigned` passes.
pub fn vouch(
    staged: &Path,
    sha256: &str,
    kind: UpdateKind,
    release: Option<&SignedRelease>,
    unsigned: bool,
    trust: Trust<'_>,
) -> Result<(), Refusal> {
    update::verify(staged, sha256)?;
    match release {
        Some(release) => check(release, sha256, &entry_for(kind, trust.target), trust),
        None if unsigned => {
            warn!(
                ?kind,
                "installing an upload no release signed, as you chose on the Mac"
            );
            Ok(())
        }
        None => Err(Refusal::Unsigned),
    }
}

/// Checks that `release` is signed with the release key, that its version
/// isn't older than this maxd's, and that it lists `sha256` under `entry`.
pub fn check(
    release: &SignedRelease,
    sha256: &str,
    entry: &str,
    trust: Trust<'_>,
) -> Result<(), Refusal> {
    let key = decode_text(trust.key)
        .and_then(|text| PublicKey::decode(&text).ok())
        .ok_or_else(|| Refusal::Manifest("this maxd's release key doesn't read".to_owned()))?;
    let signature = decode_text(&release.signature)
        .and_then(|text| Signature::decode(&text).ok())
        .ok_or(Refusal::Signature)?;
    key.verify(release.manifest.as_bytes(), &signature, false)
        .map_err(|_| Refusal::Signature)?;
    let manifest: Manifest = serde_json::from_str(&release.manifest)
        .map_err(|err| Refusal::Manifest(err.to_string()))?;
    let released = semver::Version::parse(manifest.version.trim())
        .map_err(|err| Refusal::Manifest(format!("its version: {err}")))?;
    let running = semver::Version::parse(trust.version)
        .map_err(|err| Refusal::Manifest(format!("this maxd's version: {err}")))?;
    if released < running {
        return Err(Refusal::Older {
            release: released.to_string(),
            running: running.to_string(),
        });
    }
    let sha256 = sha256.trim();
    let same = |listed: &String| listed.trim().eq_ignore_ascii_case(sha256);
    if manifest.binaries.get(entry).is_some_and(same) {
        return Ok(());
    }
    match manifest.binaries.iter().find(|(_, listed)| same(listed)) {
        Some((listed, _)) => Err(Refusal::OtherTarget {
            listed: listed.clone(),
            wanted: entry.to_owned(),
        }),
        None => Err(Refusal::Unlisted {
            entry: entry.to_owned(),
        }),
    }
}

/// The text inside base64, as `tauri signer` wraps keys and signatures.
fn decode_text(base64: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64.trim())
        .ok()?;
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests;
