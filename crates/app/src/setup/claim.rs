// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Setup's side of the question about which device a link reaches: it
//! waits for your answer before it goes on, asks before it keeps new host
//! keys under a device's name that has some, and gives the question the
//! details you judge it by.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine;
use maxhq_client::setup::{self as prepare, System};
use maxhq_client::LinkSpec;
use maxhq_protocol::ids::{DeviceId, LinkId};
use maxhq_protocol::link::{LinkClaim, LinkClaimState, Route};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use serde::Serialize;
use sha2::{Digest, Sha256};
use specta::Type;

use super::{connected_device, down_reason, Run, SetupPhase, HELLO_WITHIN};
use crate::devices::Connector;
use crate::merge::Merge;
use crate::request::CommandError;
use crate::App;

/// How long setup waits for your answer about which device a host is.
const ANSWER_WITHIN: Duration = prepare::CONFIRM_WITHIN;

/// Where you change your answer later, as the question says.
pub const WHERE_TO_CHANGE: &str = "Identity, on the device's page in the sidebar";

/// The host keys each running setup brought, by the link it set up, for
/// the question to show while it waits.
static BROUGHT: LazyLock<Mutex<BTreeMap<LinkId, Vec<String>>>> = LazyLock::new(Default::default);

fn brought() -> std::sync::MutexGuard<'static, BTreeMap<LinkId, Vec<String>>> {
    BROUGHT.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Keeps the host keys a setup brought for `link` until it ends.
pub(super) fn bring(link: LinkId, keys: &[String]) {
    brought().insert(link, keys.to_vec());
}

/// Lets go of the host keys a setup brought for `link`.
pub(super) fn done(link: LinkId) {
    brought().remove(&link);
}

/// Whether `keys` has one MaxHQ doesn't keep yet under a device's name
/// that has some already, which a setup asks you about first.
pub(super) fn adds_keys(kept: &[String], keys: &[String]) -> bool {
    !kept.is_empty() && keys.iter().any(|key| !kept.contains(key))
}

/// One side of the question: a link, where it's reached and what it was
/// seen with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct ClaimedLink {
    pub link: LinkId,
    pub name: String,
    /// The hosts it's reached at, in the order they're tried.
    pub addresses: Vec<String>,
    /// Its host keys' fingerprints, such as `ED25519 SHA256:...`: those
    /// its setup brought, or else those your own `known_hosts` has for it.
    pub fingerprints: Vec<String>,
    /// When it first reached the device, or for the link that waits, when
    /// it first named it.
    pub first_seen: Option<Timestamp>,
}

/// What the question about a held link shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct LinkClaimDetails {
    /// The link that waits for your answer.
    pub link: ClaimedLink,
    /// The link that knows the device already, when one does.
    pub known_by: Option<ClaimedLink>,
    /// The fingerprints of the host keys MaxHQ keeps under the device's
    /// name.
    pub device_keys: Vec<String>,
    /// Where you change your answer later.
    pub where_to_change: String,
}

impl<C: Connector> App<C> {
    /// Waits for the device a set-up `link` reaches. When the host keys
    /// setup brought would join others MaxHQ keeps under that device's
    /// name, it asks you first whether the host is that device, unless you
    /// just said so.
    pub(super) async fn vouched_device(
        &self,
        run: &Run<'_>,
        known_hosts: &Path,
        link: LinkId,
        keys: &[String],
    ) -> Result<DeviceId, String> {
        let mut asked = false;
        let device = self.device_on(run, link, &mut asked).await?;
        let kept = prepare::kept_host_keys(known_hosts, &device.to_string());
        if asked || !adds_keys(&kept, keys) {
            return Ok(device);
        }
        self.devices
            .ask_about(link, device)
            .map_err(|error| error.to_string())?;
        self.device_on(run, link, &mut asked).await
    }

    /// Waits until `link` reaches a device, through your answer when it
    /// waits on one, and gives the device. `asked` turns true once you
    /// said it's the device it named.
    pub(super) async fn device_on(
        &self,
        run: &Run<'_>,
        link: LinkId,
        asked: &mut bool,
    ) -> Result<DeviceId, String> {
        let mut deadline = Instant::now() + HELLO_WITHIN;
        let mut waiting = false;
        loop {
            let (device, claim) = self
                .view
                .read(|merge| (connected_device(merge, link), claim(merge, link)));
            if let Some(device) = device {
                *asked |= waiting;
                return Ok(device);
            }
            match claim.map(|claim| claim.state) {
                Some(LinkClaimState::Different) => {
                    return Err(format!(
                        "You said this host isn't the device it names, so setup stopped. \
                         You can ask again under {WHERE_TO_CHANGE}"
                    ));
                }
                Some(LinkClaimState::Asking) if !waiting => {
                    waiting = true;
                    deadline = Instant::now() + ANSWER_WITHIN;
                    run.step(SetupPhase::WaitingForAnswer);
                }
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(if waiting {
                    format!(
                        "No answer came about which device this is. The question waits under {WHERE_TO_CHANGE}"
                    )
                } else {
                    self.view
                        .read(|merge| down_reason(merge, link))
                        .unwrap_or_else(|| {
                            "The device didn't answer within a minute of setup".to_owned()
                        })
                });
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// What the question about held `link` shows.
    pub async fn link_claim_details(&self, link: LinkId) -> Result<LinkClaimDetails, CommandError> {
        let claim =
            self.view
                .read(|merge| claim(merge, link))
                .ok_or_else(|| CommandError::Failed {
                    message: "This link isn't waiting for an answer about its device".to_owned(),
                })?;
        let (spec, other, known_since) = {
            let registry = self.devices.registry();
            let spec = registry.get(link)?;
            let other = match claim.known_by {
                Some(other) => registry.get(other).ok(),
                None => None,
            };
            let known_since = match &other {
                Some(other) => registry.first_seen(other.id)?,
                None => None,
            };
            (spec, other, known_since)
        };
        let brought = brought().get(&link).cloned();
        let keys = match brought {
            Some(keys) => keys,
            None => host_keys_of(&spec).await,
        };
        let held = side(&spec, &keys, Some(claim.since));
        let known_by = match other {
            Some(other) => {
                let keys = host_keys_of(&other).await;
                Some(side(&other, &keys, known_since))
            }
            None => None,
        };
        let device_keys = prepare::paths()
            .filter(|_| !cfg!(test))
            .map(|paths| prepare::kept_host_keys(&paths.known_hosts, &claim.device.to_string()))
            .unwrap_or_default();
        Ok(LinkClaimDetails {
            link: held,
            known_by,
            device_keys: device_keys
                .iter()
                .filter_map(|key| fingerprint(key))
                .collect(),
            where_to_change: WHERE_TO_CHANGE.to_owned(),
        })
    }
}

/// The question the view shows on `link`, if any.
pub(super) fn claim(merge: &Merge, link: LinkId) -> Option<LinkClaim> {
    match merge.store().get(&EntityKey::DeviceLink(link)) {
        Some(Entity::DeviceLink(found)) => found.claim.clone(),
        _ => None,
    }
}

/// One side of the question, from its spec and host keys.
fn side(spec: &LinkSpec, keys: &[String], first_seen: Option<Timestamp>) -> ClaimedLink {
    ClaimedLink {
        link: spec.id,
        name: spec.name.clone(),
        addresses: addresses(spec),
        fingerprints: keys.iter().filter_map(|key| fingerprint(key)).collect(),
        first_seen,
    }
}

/// The hosts a link is reached at, in the order they're tried.
fn addresses(spec: &LinkSpec) -> Vec<String> {
    std::iter::once(&spec.route)
        .chain(&spec.fallbacks)
        .filter_map(|route| match route {
            Route::Ssh { host } => Some(host.clone()),
            _ => None,
        })
        .collect()
}

/// The host keys your own `known_hosts` has for a link's first host.
async fn host_keys_of(spec: &LinkSpec) -> Vec<String> {
    if cfg!(test) {
        return Vec::new();
    }
    match addresses(spec).first() {
        Some(host) => prepare::known_host_keys(&System, host).await,
        None => Vec::new(),
    }
}

/// A host key's type and fingerprint as `ssh-keygen -l` shows them, such
/// as `ED25519 SHA256:...`, from its `type base64` pair.
pub(crate) fn fingerprint(key: &str) -> Option<String> {
    let mut words = key.split_whitespace();
    let (kind, blob) = (words.next()?, words.next()?);
    let blob = STANDARD.decode(blob).ok()?;
    let digest = Sha256::digest(&blob);
    let kind = match kind {
        "ssh-ed25519" => "ED25519".to_owned(),
        "ssh-rsa" => "RSA".to_owned(),
        kind if kind.starts_with("ecdsa-") => "ECDSA".to_owned(),
        kind if kind.starts_with("sk-ssh-ed25519") => "ED25519-SK".to_owned(),
        kind if kind.starts_with("sk-ecdsa-") => "ECDSA-SK".to_owned(),
        kind => kind.to_uppercase(),
    };
    Some(format!("{kind} SHA256:{}", STANDARD_NO_PAD.encode(digest)))
}

#[cfg(test)]
mod tests;
