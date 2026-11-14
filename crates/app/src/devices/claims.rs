// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Holding a link back when its first hello names a device another link
//! already knows, until you say whether it's the same device. A hello can
//! say any device ID, so a device on your list could otherwise pass as
//! another one. A held link shows its claim and no device: what it reports
//! isn't merged as that device, and nothing meant for the device goes
//! through it.

use std::sync::Arc;

use maxhq_protocol::ids::{DeviceId, LinkId};
use maxhq_protocol::link::{DeviceLink, LinkClaim, LinkClaimState};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::time::Timestamp;

use super::{device_links, lock, Connector, Core, Devices, LinkControl};
use crate::merge::Merge;
use crate::registry::{local_link, Registry, RegistryError};
use crate::request::CommandError;

impl<C: Connector> Devices<C> {
    /// Answers the question about held `link`: the same device it named,
    /// which it reaches from then on alongside the link that knew it, or a
    /// different one, which keeps it held with a note until you ask again.
    pub fn answer_claim(&self, link: LinkId, same: bool) -> Result<(), CommandError> {
        let answered = {
            let registry = lock(&self.core.registry);
            if same {
                registry.confirm_claim(link)?.is_some()
            } else {
                registry.refuse_claim(link)?
            }
        };
        if !answered {
            return Err(not_held());
        }
        self.core.reopen(link)
    }

    /// Asks again about held `link`, after you said it's a different
    /// device.
    pub fn ask_claim_again(&self, link: LinkId) -> Result<(), CommandError> {
        if !lock(&self.core.registry).reask_claim(link)? {
            return Err(not_held());
        }
        self.core.reopen(link)
    }

    /// Forgets which device `link` reaches and what you said about it. Its
    /// next hello counts as its first, which asks you again when another
    /// link knows the device it names.
    pub fn forget_identity(&self, link: LinkId) -> Result<(), CommandError> {
        if link == local_link() {
            return Err(CommandError::Failed {
                message: "This Mac always reaches its own maxd".to_owned(),
            });
        }
        lock(&self.core.registry).forget_identity(link)?;
        self.core.reopen(link)
    }

    /// Holds `link` until you say it's `device`, as setup does before it
    /// keeps new host keys under a name that has some.
    pub fn ask_about(&self, link: LinkId, device: DeviceId) -> Result<(), CommandError> {
        let known_by = self.core.view.read(|merge| {
            device_links(merge, device)
                .map(|other| other.id)
                .find(|&other| other != link)
        });
        if lock(&self.core.registry)
            .ask_about(link, device, known_by)?
            .is_none()
        {
            return Err(not_held());
        }
        self.core.reopen(link)
    }
}

impl<L: LinkControl> Core<L> {
    /// Holds a link back when what it says about its device needs your
    /// answer first, and says whether it did. A held link shows the claim
    /// and no device.
    pub(super) fn screen(&self, id: LinkId, link: &mut DeviceLink) -> bool {
        if id == local_link() {
            return false;
        }
        let claim = match link.device {
            Some(device) => self.claim_on(id, device),
            None => lock(&self.registry).claim(id).unwrap_or_else(|error| {
                tracing::warn!(%error, link = %id, "couldn't read what a link waits on");
                None
            }),
        };
        let Some(claim) = claim else {
            return false;
        };
        link.device = None;
        link.claim = Some(claim);
        true
    }

    /// What holds `id` back now that it said it's `device`, or none once it
    /// reaches the device. A first hello reaches the device unless another
    /// link knows it already. When the registry can't say, the link waits.
    fn claim_on(&self, id: LinkId, device: DeviceId) -> Option<LinkClaim> {
        let result = (|| {
            if let Some(decided) = decided(&lock(&self.registry), id, device)? {
                return Ok(decided);
            }
            let seen: Option<LinkId> = self.view.read(|merge| {
                device_links(merge, device)
                    .map(|other| other.id)
                    .find(|&other| other != id)
            });
            let kept = self.kept_links(device, id);
            // Asked again under one hold, so two first hellos for the same
            // device can't both get it.
            let registry = lock(&self.registry);
            if let Some(decided) = decided(&registry, id, device)? {
                return Ok(decided);
            }
            let known_by = match seen {
                Some(other) => Some(other),
                None => registry.known_elsewhere(device, id, &kept)?,
            };
            let Some(other) = known_by else {
                registry.keep_device(id, device)?;
                return Ok(None);
            };
            tracing::warn!(link = %id, %device, known_by = %other, "a link names a device another link knows, so it waits for your answer");
            let held = registry.hold(id, device, Some(other))?;
            Ok(Some(held.unwrap_or_else(|| asking(device, Some(other)))))
        })();
        result.unwrap_or_else(|error: RegistryError| {
            tracing::warn!(%error, link = %id, "couldn't check which device a link reaches, so it waits");
            Some(asking(device, None))
        })
    }

    /// Opens a link again from scratch, so the view and the client take
    /// what the registry now says it reaches.
    fn reopen(self: &Arc<Self>, link: LinkId) -> Result<(), CommandError> {
        let spec = lock(&self.registry).get(link)?;
        self.drop_links(&[link]);
        self.open(spec);
        self.sync_peers();
        Ok(())
    }
}

/// What the registry already says for `id` naming `device`: Some(None)
/// when it reaches a device, Some(claim) when it waits on you for this
/// device or you said it's a different one, and None when nothing's
/// decided.
fn decided(
    registry: &Registry,
    id: LinkId,
    device: DeviceId,
) -> Result<Option<Option<LinkClaim>>, RegistryError> {
    if registry.known_device(id)?.is_some() {
        return Ok(Some(None));
    }
    Ok(registry
        .claim(id)?
        .filter(|claim| claim.state == LinkClaimState::Different || claim.device == device)
        .map(Some))
}

/// A question about `device` from now.
fn asking(device: DeviceId, known_by: Option<LinkId>) -> LinkClaim {
    LinkClaim {
        device,
        known_by,
        state: LinkClaimState::Asking,
        since: Timestamp::now(),
    }
}

/// Whether the view holds link `id` back.
pub(super) fn held(merge: &Merge, id: LinkId) -> bool {
    matches!(
        merge.store().get(&EntityKey::DeviceLink(id)),
        Some(Entity::DeviceLink(link)) if link.claim.is_some()
    )
}

fn not_held() -> CommandError {
    CommandError::Failed {
        message: "This link isn't waiting for an answer about its device".to_owned(),
    }
}

#[cfg(test)]
mod tests;
